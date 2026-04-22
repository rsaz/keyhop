# Release Process for keyhop

This document describes how to release a new version of keyhop and update the Microsoft Store submission.

## Creating a New Release

1. **Update version** in `Cargo.toml`:
   ```toml
   version = "0.3.0"
   ```

2. **Update** `CHANGELOG.md` with release notes

3. **Build the release binary:**
   ```powershell
   cargo build --release
   ```
   Binary will be at: `target\release\keyhop.exe`

4. **Test the binary** to ensure it works

5. **Commit and tag:**
   ```powershell
   git add .
   git commit -m "Release v0.3.0"
   git tag v0.3.0
   git push origin main --tags
   ```

6. **Create GitHub release:**
   ```powershell
   gh release create v0.3.0 --title "v0.3.0 - Feature Name" --notes "See CHANGELOG.md"
   ```
   Or use the GitHub web interface: https://github.com/rsaz/keyhop/releases/new

7. **Upload the binary** to the GitHub release (if not using the automated workflow)

## Getting the Direct Download URL for Microsoft Store

After creating the release, you need to get the direct CDN URL (not the redirect URL):

1. **Get the redirect URL** from GitHub:
   ```
   https://github.com/rsaz/keyhop/releases/download/v0.3.0/keyhop.exe
   ```

2. **Get the direct CDN URL** using curl:
   ```powershell
   curl -sI "https://github.com/rsaz/keyhop/releases/download/v0.3.0/keyhop.exe" | findstr Location
   ```

3. **Copy the Location URL** - it will look like:
   ```
   https://release-assets.githubusercontent.com/github-production-release-asset/...
   ```

4. **Extract the permanent part** - Remove the query parameters with expiration:
   - The URL has JWT tokens that expire
   - You need to use the base redirect URL or find the permanent CDN URL pattern

**Note:** GitHub's CDN URLs contain temporary tokens, so you'll need to update the Microsoft Store submission URL with each new release version anyway.

## Alternative: Use the Redirect URL

Microsoft Store may accept the redirect URL despite the warning:
```
https://github.com/rsaz/keyhop/releases/download/v0.3.0/keyhop.exe
```

Update this URL in Microsoft Store submission for each new version.

## Simplified Workflow

For each release:
1. Build: `cargo build --release`
2. Tag and create GitHub release
3. Upload binary to GitHub release
4. Update Microsoft Store submission with the new version's GitHub URL
