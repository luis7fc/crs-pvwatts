# PVWatts Tool on a Mac

The Mac version is built separately from the Windows exe (see
`.github/workflows/build-macos.yml`). It runs on both Apple-chip and Intel Macs.
It works the same way as the Windows version: it opens in the browser at
`http://127.0.0.1:8787`.

## Building a release

1. Commit to `herb`, then tag and push the tag:
   `git tag mac-v2.1.2 && git push origin mac-v2.1.2`
2. GitHub → **Actions** → **build-macos** → the run for that tag → **Artifacts**.
   Download `pvwatts_tool_mac_mac-v2.1.2`. It's a zip that holds the real zip.
3. Share the inner `pvwatts_tool_mac_mac-v2.1.2.zip` with the Mac user.

## One-time setup on the Mac

1. **Connect the I: drive.** In Finder, choose **Go → Connect to Server**, enter
   `smb://CRS-SERVER1/Shares`, sign in, and tick **Remember this password in my
   keychain**. It appears as `/Volumes/Shares`. Reconnect it after a restart
   (or add it under System Settings → General → Login Items).
2. **Unzip** the tool into its own folder, e.g. `Documents/PVWatts`. The tool
   saves its settings file (`pvwatts.env`) next to itself.
3. **First open:** double-click `pvwatts_tool`. macOS blocks it because it's
   unsigned. Go to **System Settings → Privacy & Security**, scroll down, and
   click **Open Anyway**, then confirm. This happens once per new version.
4. A Terminal window opens and the browser opens the tool. **Leave the
   Terminal window open** while using it, and close it to quit.
5. Paste the sidecar key in ⚙ Settings and sign in with your Creatio login.
   When macOS asks to use the keychain, click **Always Allow**.

## How I: drive paths work

Saved paths stay in the Windows form (`I:\...`) so Windows and Mac users can
share them. The Mac copy translates `I:\` to `/Volumes/Shares/` when reading,
and back when writing (`src/share.rs`). If Finder connected the share under a
different name, such as `Shares-1`, the tool finds it. Set
`PVWATTS_SHARE_MOUNT="/Volumes/<name>"` in `pvwatts.env` to force a location.

## Updating

Replace `pvwatts_tool` with the new one. The key and password stay in the
keychain. macOS may ask for **Open Anyway** and **Always Allow** again.
