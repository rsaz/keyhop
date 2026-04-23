# Microsoft Store submission for keyhop

This is the operations runbook for the keyhop **MSI/EXE Win32** listing
on the Microsoft Store. It captures the exact Partner Center field
values, why they're set the way they are, and what to do when the
automated validator complains.

For the release-cutting steps that *produce* the artifact you submit
here, see [`../RELEASE_PROCESS.md`](../RELEASE_PROCESS.md). For code
signing (a Store recommendation; not yet implemented), see
[`CODE_SIGNING.md`](CODE_SIGNING.md).

## Why MSI/EXE and not MSIX

The Store accepts three Win32 packaging formats: **MSI**, **EXE**, and
**MSIX**. We ship the WiX-generated MSI for one reason: keyhop installs a
**system-wide low-level keyboard hook** and registers a global hotkey
through `RegisterHotKey`. MSIX containers run apps inside an AppContainer
sandbox that blocks both APIs unless you declare
`runFullTrust` — and `runFullTrust` requires Microsoft to grant you the
restricted capability one-by-one, with a justification review. The MSI
path skips that entire dance, ships per-machine, and is the right shape
for a system utility anyway.

## Hosting the MSI: Cloudflare R2 (with GitHub Releases as the source of truth)

Microsoft Store needs **one stable URL** that returns the MSI bytes
without redirects through expiring CDN tokens. We use **Cloudflare R2**
behind the custom domain `dl.expresso-ts.com`.

| Channel | URL pattern | Used for |
|---|---|---|
| **R2** *(canonical for Partner Center)* | `https://dl.expresso-ts.com/release/v<version>/Keyhop-<version>-x86_64.msi` | The URL pasted into Partner Center |
| **GitHub Releases** *(canonical for humans)* | `https://github.com/rsaz/keyhop/releases/download/v<version>/Keyhop-<version>-x86_64.msi` | The URL referenced from the README, source of bytes mirrored to R2 |

R2 is configured for the `keyhop` bucket on the ExpressoTS Cloudflare
account:

- Custom domain `dl.expresso-ts.com`, **TLS 1.2+ enforced**
- `r2.dev` managed subdomain **disabled** (so the bucket is only reachable via the custom domain)
- Layout: `release/v<version>/Keyhop-<version>-x86_64.msi` + `.sha256` sidecar
- Per-release subfolders kept indefinitely (Partner Center re-validates the URL+SHA256 pair until each submission is retired — see [`../RELEASE_PROCESS.md`](../RELEASE_PROCESS.md#4-after-certification))

### Why not just point Partner Center at the github.com URL?

The github.com URL technically works — Partner Center will follow the
302 redirect to GitHub's signed CDN — but Partner Center has been
observed to occasionally cache the *resolved* expiring URL instead of
re-following the redirect, which then 403s mid-cycle and flags the
listing. R2 has no expiring URL anywhere in the chain, which removes
the entire failure class.

It's also visibly an `expresso-ts.com` URL, not a generic GitHub CDN
hostname, which is a small but real trust signal for human reviewers
clicking through the package URL during certification.

### Hosting alternatives, ranked

Documented so future-you can make an informed call if R2 ever stops
working for us. **`$0` numbers assume keyhop's actual traffic** (~1 MB
MSI, a few hundred Store re-validations + user downloads per month —
well under any free tier).

| Host | Free tier | Stable URL pattern | Effort per release | Real cost for keyhop |
|---|---|---|---|---|
| **Cloudflare R2** *(current)* | 10 GB storage, **unlimited free egress** forever, 1 M Class A ops/mo | `dl.expresso-ts.com/release/v<version>/...` | one upload to R2 per release (manual today, automation pending) | **$0** |
| **GitHub Releases** *(also live, as bytes source)* | unlimited public, no egress cap | `github.com/<owner>/<repo>/releases/download/<tag>/<file>` | already automated in `release.yml` | **$0** |
| **Supabase public bucket** | 1 GB storage, 5 GB egress/mo | `<project>.supabase.co/storage/v1/object/public/<bucket>/<file>` | extra `supabase storage cp` step | $0 (well under 5 GB) |
| **Backblaze B2** | 10 GB storage, 1 GB egress/day | `f<NNN>.backblazeb2.com/file/<bucket>/<file>` | extra `b2 upload-file` step | ~$0; pennies/mo if exceeded |
| **AWS S3** | 5 GB / 12 months only, then pay | `<bucket>.s3.<region>.amazonaws.com/<file>` | extra `aws s3 cp` | ~$0.09/GB egress after free tier expires |

**On Supabase specifically:** a public bucket *would* work technically —
URL stable, no JWT, well inside the 5 GB/mo egress free tier. But it
adds an upload step for zero benefit over R2. Use Supabase only if we
ever need auth-gated downloads (signed URLs / RLS) — irrelevant for a
public installer.

### Future automation

The R2 upload is currently a manual dashboard drag-and-drop. To
automate it inside `.github/workflows/release.yml`, we need:

1. A bucket-scoped R2 API token (Cloudflare dashboard → R2 → **Manage R2 API Tokens** → permissions `Object Read & Write`, scoped to the `keyhop` bucket only)
2. Four GitHub Actions repository secrets: `R2_ACCOUNT_ID`,
   `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY`, `R2_PUBLIC_BASE_URL=https://dl.expresso-ts.com`
3. A new step in `release.yml` between "Build MSI installer" and
   "Attach binaries to GitHub Release":
   ```yaml
   - name: Mirror MSI to Cloudflare R2
     env:
       AWS_ACCESS_KEY_ID:     ${{ secrets.R2_ACCESS_KEY_ID }}
       AWS_SECRET_ACCESS_KEY: ${{ secrets.R2_SECRET_ACCESS_KEY }}
       AWS_DEFAULT_REGION:    auto
     run: |
       VERSION=${GITHUB_REF_NAME#v}
       MSI=target/wix/Keyhop-${VERSION}-x86_64.msi
       sha256sum "$MSI" | awk '{print $1}' > "$MSI.sha256"
       aws s3 cp "$MSI"        "s3://keyhop/release/v${VERSION}/" \
         --endpoint-url https://${{ secrets.R2_ACCOUNT_ID }}.r2.cloudflarestorage.com
       aws s3 cp "$MSI.sha256" "s3://keyhop/release/v${VERSION}/" \
         --endpoint-url https://${{ secrets.R2_ACCOUNT_ID }}.r2.cloudflarestorage.com
   ```
   (R2 speaks the S3 API, so the AWS CLI works without `wrangler` if
   we'd rather not add another dependency to the runner.)

Until that's in place, the dashboard upload is documented in
[`../RELEASE_PROCESS.md`](../RELEASE_PROCESS.md#2-mirror-the-msi-to-cloudflare-r2).

## Partner Center field values

Open the keyhop product in
[Partner Center → Apps and games](https://partner.microsoft.com/dashboard/apps-and-games/overview)
→ **New submission** → **Packages**, paste the package URL, then expand
the package row to fill in the installer details:

### Package URL

```text
https://dl.expresso-ts.com/release/v0.3.0/Keyhop-0.3.0-x86_64.msi
```

Bump the version on every release. The MSI must be uploaded to R2
*before* you submit (see
[`../RELEASE_PROCESS.md § 2`](../RELEASE_PROCESS.md#2-mirror-the-msi-to-cloudflare-r2));
Partner Center records the SHA256 at submission time and re-validates
the URL+hash pair on a periodic basis until the submission is retired.

**Never paste a `release-assets.githubusercontent.com/...?jwt=...&se=...`
URL** even if you grab it from a `curl -I` of the github.com URL —
those have a 5-minute SAS expiry. The R2 URL has no expiring tokens
anywhere in the chain, which is the whole reason we mirror to it.

### Installer details

| Field | Value | Why |
|---|---|---|
| **Installer type** | `MSI` (auto-detected) | Verify it's not `EXE`. If it's wrong, the validator won't pass `/qn` and silent-install fails. |
| **Silent install command** | `/qn` | Maps to `msiexec /i <pkg> /qn`. Required by Store policy 10.2.9. |
| **Silent install with progress** | `/qb!-` | Optional. Hides the cancel button; shows a progress bar without user interaction. |
| **Custom install command** | *(blank)* | The MSI handles install; no wrapper needed. |
| **Default install location** | *(blank)* | The MSI installs to `%ProgramFiles%\Keyhop\` — Windows Installer reports this back. |
| **Product code** | auto-detected | Partner Center reads it from the MSI's `Product/@Id`. Don't override. |
| **Upgrade code** | `49555F40-7E07-468A-8307-AE8FCEEB63DA` | Matches `wix/main.wxs`. **Never change this** — it's how Windows recognizes upgrades vs. fresh installs across versions. |
| **Add/Remove Programs entry** | auto-populated | Comes from the MSI's `<Product>` + `<Package>` attributes (Name=`Keyhop`, Manufacturer=`Richard Zampieri`, Version=`<var.Version>`). |
| **Code signature** | *(none today)* | See [`CODE_SIGNING.md`](CODE_SIGNING.md). Currently flagged as a recommendation, not a blocker. |

### Listing metadata

| Field | Value |
|---|---|
| App name | `Keyhop` |
| Publisher | `Richard Zampieri` |
| Category | `Productivity` (sub: `Personal finance` is wrong; pick `Personal assistance` or `Utilities & tools` if Productivity isn't fine-grained enough) |
| Privacy policy URL | `https://github.com/rsaz/keyhop/blob/main/PRIVACY_POLICY.md` |
| Support contact info | `https://github.com/rsaz/keyhop/issues` |
| Age rating | Fill in the IARC questionnaire — keyhop has no UGC, no ads, no in-app purchase. |

## Validation failures and how to fix them

Partner Center runs four automated checks. Every check fires whether or
not the previous one passed, but **silent-install failure cascades** —
ARP and Bundleware checks both depend on the install actually completing.

### 1. Silent install check

> *Your app does not install silently which is a violation of Microsoft Store Policy 10.2.9.*

Causes, in order of likelihood:

1. **Silent install command empty in Partner Center** → MS runs the MSI
   with no args → WiX UI shows up → headless validator times out and
   reports `800704C7 The operation was canceled by the user`. Fix: set
   the field to `/qn`.
2. **Installer type misdetected as `EXE`** → MS doesn't pass `/qn` even
   if you set it on an MSI. Fix: switch the type to `MSI` in Partner
   Center's installer details.
3. **MSI itself isn't silent-install clean** → run `msiexec /i ... /qn`
   locally and watch for any UI dialog (EULA, custom action prompt,
   error). If you see one, fix `wix/main.wxs` first.

The smoking gun is the **WPM report file** Partner Center gives you on a
failure. Look for the line `[CLI ] Installer args:` — if it's empty,
it's #1 or #2 above.

### 2. Add/Remove Programs entry check

> *We could not identify the app name and the publisher name that your app has added in the add or remove programs.*

If silent install just failed, this will *always* also fail (no install
→ no ARP entry to inspect). Fix the silent install first, then re-run
validation; this normally clears itself.

If silent install succeeded but ARP still fails, inspect locally:

```powershell
msiexec /i target\wix\Keyhop-0.3.0-x86_64.msi /qn
Get-ItemProperty 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*' |
    Where-Object { $_.DisplayName -eq 'Keyhop' } |
    Select-Object DisplayName, Publisher, DisplayVersion, UninstallString
```

You should see `Keyhop`, `Richard Zampieri`, the version, and an
`UninstallString` like `MsiExec.exe /X{...}`. If any are blank, the MSI
is wrong — check `<Product>` and `<Package>` in `wix/main.wxs`.

### 3. Bundleware check

> *Your app should only add a single entry to the programs list.*

Same root cause as ARP — if silent install fails, this also fails.
Independently, this check counts ARP entries; the keyhop MSI registers
exactly one. We don't bundle anything. If this ever fails on its own,
something is *very* wrong.

### 4. Code sign check

> *Your app does not have a digital signature ... SHA256 or higher code sign certificate.*

This is a **recommendation**, not a hard fail today. The submission
proceeds. To clear the warning, follow [`CODE_SIGNING.md`](CODE_SIGNING.md)
and re-cut the release.

## Useful references

- [Microsoft Store Policy 10.2 — Security](https://learn.microsoft.com/en-us/windows/apps/publish/store-policies#102-security)
- [Manual package validation for MSI/EXE](https://learn.microsoft.com/en-us/windows/apps/publish/publish-your-app/msi/manual-package-validation)
- [Microsoft Trusted Signing](https://learn.microsoft.com/en-us/azure/trusted-signing/) — the path we're taking for code signing
- [WiX 3 documentation](https://wixtoolset.org/docs/v3/) — what `wix/main.wxs` is built against
