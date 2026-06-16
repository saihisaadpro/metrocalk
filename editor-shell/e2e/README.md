# editor-shell E2E (WebdriverIO + tauri-driver)

Real-`.exe` end-to-end tests: WebdriverIO drives the **WebView2 DOM** of the packaged Tauri app through
`tauri-driver`. Because the editor panel is DOM and the viewport is a transparent `<div>` over the
native wgpu surface, clicking that div fires the real native pick — so the full live round-trip
(launch → connect → reveal → bind → undo → **viewport pick** → edit) is verified automatically. This is
the harness that turned the viewport-pick bug from a multi-round-trip guessing game into a one-command
reproduction. (Tauri WebDriver works on Windows/Linux; macOS desktop is unsupported.)

## One-time setup

1. **tauri-driver:** `cargo install tauri-driver --locked`
2. **msedgedriver matching the WebView2 runtime** (check the runtime version first, they must match or
   the session hangs):
   ```powershell
   # WebView2 runtime version:
   (Get-ItemProperty "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}").pv
   # download the matching driver into ./.driver/ :
   Invoke-WebRequest "https://msedgedriver.microsoft.com/<VERSION>/edgedriver_win64.zip" -OutFile .driver\d.zip
   Expand-Archive .driver\d.zip .driver -Force
   ```
3. **deps:** `npm install`
4. **build the app under test:** from the repo root,
   `cargo build --release --manifest-path editor-shell/src-tauri/Cargo.toml`

## Run

```powershell
# NOTE: run wdio via node directly — the repo path contains " & ", which breaks npm's cmd .bin shim.
node "node_modules\@wdio\cli\bin\wdio.js" run wdio.conf.js
```

`wdio.conf.js` starts `tauri-driver` (pointed at `./.driver/msedgedriver.exe`), launches the release
binary, and deletes the persistence log first so each run starts on a clean, deterministically-seeded
scene. Specs are in `./specs`.

`node_modules/`, `.driver/`, and `package-lock.json` are gitignored — re-run the setup to restore them.
