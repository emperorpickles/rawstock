Unsigned M1 alpha of the rawstock desktop app. It is not code signed or
notarized, so Windows and macOS warn before the first launch. Each file has
a `.sha256` next to it; check it before installing.

**Windows 10/11.** Run `rawstock_*_x64-setup.exe`. If SmartScreen shows
"Windows protected your PC", choose **More info**, then **Run anyway**.

**macOS 13 or later (Apple Silicon and Intel).** Open the `.dmg` and drag
rawstock to Applications. The first launch is blocked because the app is
unsigned. Open **System Settings > Privacy & Security**, find the rawstock
message, and choose **Open Anyway**. Or, in Terminal:

```sh
xattr -dr com.apple.quarantine /Applications/rawstock.app
```

The installer registers `.raf` and `.dng`, so a RAW file can be opened from
Explorer or Finder with rawstock. Edits are saved next to each photo as
`<file>.rawstock.toml`; the RAW file is never written.
