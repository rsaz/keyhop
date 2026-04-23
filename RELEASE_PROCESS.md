# Release Process for keyhop

How to cut a new keyhop release and refresh the Microsoft Store submission.

For the field-by-field Partner Center configuration (silent install switch,
ARP fields, code-sign status), see [`docs/MICROSOFT_STORE.md`](docs/MICROSOFT_STORE.md).
For the signing pipeline, see [`docs/CODE_SIGNING.md`](docs/CODE_SIGNING.md).

## Two distribution channels, one source of truth

Every keyhop release ships the same MSI bytes through two independent
channels:

| Channel | URL pattern | Purpose | Notes |
|---|---|---|---|
| **Cloudflare R2** *(canonical for Microsoft Store)* | `https://dl.expresso-ts.com/release/v<version>/Keyhop-<version>-x86_64.msi` | The URL pasted into Partner Center | Stable hostname, no expiring tokens, free egress, TLS 1.2+ enforced |
| **GitHub Releases** *(canonical for direct downloads)* | `https://github.com/rsaz/keyhop/releases/download/v<version>/Keyhop-<version>-x86_64.msi` | The URL referenced from the README and CHANGELOG, source of bytes for R2 | Per-tag immutable, also hosts `keyhop.exe` portable build |

The same SHA256-identical MSI lives at both URLs. R2 is the URL Microsoft
Store stores in the submission manifest; GitHub Releases is the URL
humans click from the README. Mismatched bytes between the two would be
a bug — `cargo script verify-r2` (Section 4 below) catches it
automatically.

## 1. Cut the release

1. **Bump the version** in `Cargo.toml`:
   ```toml
   version = "0.3.0"
   ```

2. **Update `CHANGELOG.md`** — move the unreleased section under a dated
   `## [X.Y.Z] - YYYY-MM-DD` heading and add a fresh `## [Unreleased]`
   placeholder above it.

3. **Build and smoke-test the binary:**
   ```powershell
   cargo script release
   .\target\release\keyhop.exe
   ```

4. **Build the MSI** (requires WiX Toolset 3 — `scoop install wixtoolset3`):
   ```powershell
   cargo script msi
   ```
   Produces `target\wix\Keyhop-<version>-x86_64.msi`. Verify silent install
   locally before publishing:
   ```powershell
   msiexec /i target\wix\Keyhop-0.3.0-x86_64.msi /qn
   ```
   The install must complete with **only a UAC prompt** — no installer UI.
   Confirm the ARP entry shows up under
   `Settings → Apps → Installed apps → Keyhop`, then uninstall it.

5. **Commit, tag, and push:**
   ```powershell
   git add .
   git commit -m "Release v0.3.0"
   git tag v0.3.0
   git push origin main --tags
   ```

6. **Publish the GitHub Release.** The `.github/workflows/release.yml`
   workflow fires on tag push, builds both `keyhop.exe` and the MSI, and
   attaches them to the release via `softprops/action-gh-release@v3`. If
   you ever need to do it manually:
   ```powershell
   gh release create v0.3.0 --title "v0.3.0 — short headline" --notes-file CHANGELOG.md
   gh release upload v0.3.0 target\release\keyhop.exe target\wix\Keyhop-0.3.0-x86_64.msi
   ```

## 2. Mirror the MSI to Cloudflare R2

The Microsoft Store submission requires a stable URL that Cloudflare's
JWT-expiring CDN URLs (the `release-assets.githubusercontent.com` form)
cannot provide. We mirror to R2 specifically for that purpose.

> **Why not just point Partner Center at the github.com URL?** It
> technically works (GitHub's 302 redirect is followed by the validator),
> but Partner Center has been observed to occasionally cache the
> *resolved* expiring URL instead of re-following the redirect, leading
> to mid-cycle 403 failures. R2 has no expiring URLs in the chain at
> all, which removes the entire failure class.

### Generate the SHA256 sidecar

```powershell
$msi = "target\wix\Keyhop-0.3.0-x86_64.msi"
(Get-FileHash $msi -Algorithm SHA256).Hash | Set-Content "$msi.sha256"
```

### Upload via Cloudflare dashboard (current manual flow)

1. [Cloudflare dashboard](https://dash.cloudflare.com) → **R2 Object Storage** → bucket **`keyhop`**
2. Navigate to `release/`. If a folder named `v<version>` does not yet
   exist, click **+ Add directory** → name it (e.g. `v0.3.0`) → Create.
3. Double-click into `v<version>/`. Breadcrumb should read
   `keyhop / release / v<version> /`.
4. **Upload** → drag in **both** files:
   - `Keyhop-<version>-x86_64.msi`
   - `Keyhop-<version>-x86_64.msi.sha256`

R2 enforces TLS 1.2+ on the custom domain (`dl.expresso-ts.com`); we set
that explicitly when the bucket was provisioned.

> **Future automation.** Once we issue a bucket-scoped R2 API token and
> add it to GitHub Actions secrets (`R2_ACCOUNT_ID`,
> `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY`), `release.yml` can do this
> upload step for us via `wrangler r2 object put`. Tracked in
> [`docs/MICROSOFT_STORE.md`](docs/MICROSOFT_STORE.md#future-automation).

### Verify the upload

From any PowerShell (no credentials needed — this hits the public URL):

```powershell
cargo script verify-r2 --env VERSION=0.3.0
```

This script downloads the MSI from both GitHub Releases and R2,
SHA256-hashes both, and confirms they match. If the hashes don't match,
something corrupted the upload — re-upload before continuing.

## 3. Update the Microsoft Store submission

1. Open the keyhop product in [Microsoft Partner Center](https://partner.microsoft.com/dashboard/apps-and-games/overview).
2. Start a new submission → **Packages**.
3. Replace the package URL with the new R2 URL:
   ```text
   https://dl.expresso-ts.com/release/v0.3.0/Keyhop-0.3.0-x86_64.msi
   ```
4. Verify the rest of the package metadata (silent install switch,
   installer type, ARP details) — see
   [`docs/MICROSOFT_STORE.md`](docs/MICROSOFT_STORE.md) for the full
   field list.
5. Submit for certification.

## 4. After certification

- Update the README install snippet's version number if the major/minor
  changed (`msiexec /i Keyhop-X.Y.Z-x86_64.msi /qn`).
- If signing was added in this release, remove the "unsigned binary"
  note from the release template per `docs/CODE_SIGNING.md`.
- **Do not delete** the previous version's R2 folder until Microsoft
  Partner Center has confirmed the new submission is live and the old
  submission is retired. Partner Center periodically re-validates the
  *previous* submission's URL+SHA256 pair until the rollover is
  complete; deleting the old MSI early can flag the old listing as
  tampered.
