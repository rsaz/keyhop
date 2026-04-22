# Privacy Policy for keyhop

**Last Updated:** April 22, 2026

## Overview

keyhop is a keyboard navigation tool that runs entirely on your local computer. We are committed to protecting your privacy.

## Information We Collect

keyhop does **NOT** collect, transmit, or share any personal information. The application:

- Does not connect to the internet
- Does not collect or transmit user data
- Does not track your activity
- Does not use analytics or telemetry
- Does not contain advertisements

## Local Data Storage

keyhop stores the following data locally on your device:

1. **Configuration File** (`%APPDATA%\keyhop\config.toml`):
   - Your customized hotkey settings
   - Hint alphabet preferences
   - Color and opacity preferences
   - Startup launch preference

2. **Log Files** (`%LOCALAPPDATA%\keyhop\keyhop.log`):
   - Application diagnostic logs
   - Error messages and debugging information
   - No personal or sensitive information is logged

3. **Windows Registry** (optional):
   - If you enable "Launch at Windows startup", a registry entry is created in `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
   - This entry only contains the path to the keyhop executable

All data remains on your computer and is never transmitted elsewhere.

## Permissions and Access

keyhop requires the following system permissions:

- **UI Automation API Access**: To read UI element information from applications for hint label generation
- **Global Hotkey Registration**: To respond to your configured keyboard shortcuts
- **Display Overlay**: To show hint labels on screen

These permissions are used solely for the application's core functionality and do not involve data collection.

## Data Sharing

keyhop does not share any data with third parties because it does not collect any data.

## Data Security

Since all data is stored locally on your device:
- You have full control over your data
- You can delete the configuration file at any time
- Uninstalling keyhop removes all associated data

## Children's Privacy

keyhop does not collect any personal information from anyone, including children under 13.

## Changes to This Privacy Policy

We may update this Privacy Policy from time to time. Any changes will be posted in this document with an updated "Last Updated" date.

## Open Source

keyhop is open source software. You can review the complete source code at:
https://github.com/rsaz/keyhop

## Contact

If you have questions about this Privacy Policy, please open an issue at:
https://github.com/rsaz/keyhop/issues

## Your Rights

As keyhop does not collect personal data, typical data protection regulations (GDPR, CCPA, etc.) regarding data access, deletion, and portability do not apply. However, you maintain complete control over the local configuration and log files stored on your device and can delete them at any time.

---

**Summary**: keyhop is a privacy-focused, local-only application that does not collect, transmit, or share any personal information. All configuration and logs remain on your device.
