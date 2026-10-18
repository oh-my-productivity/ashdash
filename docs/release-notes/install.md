## Install

Requirements: macOS 12 or newer, Apple Silicon or Intel (the DMG is universal).

1. Download `Ashdash-@VERSION@.dmg` and `Ashdash-@VERSION@.dmg.sha256` from this page. Check the download: `cd ~/Downloads && shasum -a 256 -c Ashdash-@VERSION@.dmg.sha256` should print `OK`.
2. Open the DMG and drag **Ashdash** to **Applications**.
3. While the build is signed *ad hoc* and not notarized (see the banner at the top of this page when it applies), macOS Gatekeeper blocks the first launch. Do **one** of these:
   - on **macOS 15 and later**: click *Done*, open System Settings > Privacy & Security, press **Open Anyway** next to "Ashdash was blocked to protect your Mac", then open Ashdash again and press **Open Anyway** in the second dialog; or
   - in Terminal, any macOS: `xattr -dr com.apple.quarantine /Applications/Ashdash.app`, then open it normally; or
   - on macOS 14 and earlier: right-click **Ashdash** in Applications, choose **Open**, then **Open** again.
4. Press **Option+Shift+Space**. Ashdash lives in the menu bar and has no Dock icon.

Upgrading: quit Ashdash and replace the app; your data in `~/Library/Application Support/ashdash` stays. Uninstall: quit, delete the app, and delete `~/Library/Application Support/ashdash` and `~/Library/Caches/ashdash`.
Documentation, source and issues: @REPO_URL@
