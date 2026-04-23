# Code signing the MSI and EXE

Microsoft Store Policy 10.2.9 has a code-sign **recommendation** (not a hard
fail today, but expected to become one): every executable and installer
shipped through the Store must be signed with a SHA-256 or higher
certificate. Unsigned installers also trigger Windows SmartScreen's "unknown
publisher" warning, which scares away most users on first install.

This document is the reference for adding signing to the keyhop release
pipeline once we have a certificate. **No signing happens today** — the v0.3.0
release is unsigned. This is fine for the GitHub Releases distribution
channel; it is the blocker for the Store submission.

## Options for getting a certificate

| Option | Cost | What it does to SmartScreen | Setup effort |
|---|---|---|---|
| **Microsoft Trusted Signing** (recommended) | ~$10/month, individual-friendly | Reputation inherited from Microsoft's CA — clean install on day one | Low. Azure-hosted, no HSM to ship around. |
| Sectigo / DigiCert OV cert | ~$200–$400/year | Builds reputation slowly via SmartScreen telemetry; first ~10–100 installs still warn | Medium. Cert lives in a USB HSM you have to plug in to sign. |
| Sectigo / DigiCert EV cert | ~$300–$500/year | Instant SmartScreen reputation | High. Strict identity verification, EV-only HSM, no exporting the key. |
| Self-signed | Free | SmartScreen blocks every install | Pointless for distribution. |

For an individual maintainer (you, today), **[Microsoft Trusted Signing](https://learn.microsoft.com/en-us/azure/trusted-signing/)** is the obvious pick: monthly billing, no HSM, signs from a GitHub Action via federated identity (no long-lived secrets in the repo).

## Workflow once a Trusted Signing account exists

1. Create the Trusted Signing account in Azure.
2. Create a **certificate profile** (this is the actual signing identity; one profile per published product is fine).
3. Configure **federated credentials** between the GitHub repo and the Azure AD app — this lets the Action pull a short-lived signing token without storing a secret.
4. Add to `.github/workflows/release.yml` between the `Build MSI installer` step and the `Attach binaries to GitHub Release` step:

```yaml
- name: Sign keyhop.exe and MSI with Trusted Signing
  uses: azure/trusted-signing-action@v0
  with:
    azure-tenant-id: ${{ secrets.AZURE_TENANT_ID }}
    azure-client-id: ${{ secrets.AZURE_CLIENT_ID }}
    azure-client-secret: ${{ secrets.AZURE_CLIENT_SECRET }}    # or federated
    endpoint: https://eus.codesigning.azure.net/
    trusted-signing-account-name: keyhop-signing
    certificate-profile-name: keyhop-public
    files-folder: target
    files-folder-filter: exe,msi
    files-folder-recurse: true
    file-digest: SHA256
    timestamp-rfc3161: http://timestamp.acs.microsoft.com
    timestamp-digest: SHA256
```

That single step signs **both** `target\release\keyhop.exe` and `target\wix\Keyhop-*.msi`.

5. Validate locally with:
   ```powershell
   signtool.exe verify /pa /v Keyhop-0.3.0-x86_64.msi
   ```

## Until the cert is in place

- The Microsoft Store submission for v0.3.0 will pass the silent-install,
  ARP, and bundleware checks (the MSI handles those) but flag the code-sign
  recommendation. The Store currently allows the submission to proceed with
  the warning.
- The GitHub Release downloads will trigger SmartScreen's "Windows protected
  your PC" dialog on first install on a fresh machine. Users have to click
  "More info" → "Run anyway". Document this in the release notes.

## When we add signing

Update `CHANGELOG.md` under the release that ships it, and remove the
"unsigned binary" note from the GitHub release template.
