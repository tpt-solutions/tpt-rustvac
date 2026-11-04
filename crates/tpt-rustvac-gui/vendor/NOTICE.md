# Third-party notice: WebView2Loader.dll

`WebView2Loader.dll` (x64) is compiled into `rustvac-gui.exe`. It is the loader for the
Microsoft Edge WebView2 runtime, which the window uses to show its page.

- File: `WebView2Loader.dll`, file version 1.0.3800.47, x64
- SHA-256: `86545b66cdb0603bc26b626fb9ad610cb6e71f28d468f5ea66df23b03dda96d5`
- Copied from the `webview2-com-sys` 0.39.1 crate (crates.io), `x64/` directory.
- Copyright (c) Microsoft Corporation. Redistributed under the Microsoft WebView2 SDK
  licence. The licence terms must be copied into the release notices (see `todo.md`,
  "WebView2 loader licence notice for releases").
- The project previously referenced the 1.0.4312-prerelease build (SHA-256
  `909828eb2922694b156ae367751466722fe148a1b151245bbc8a80a24dc32645`). That file was not
  available when this copy was made. Replace this file with that build and update the hash
  above if it is recovered.
