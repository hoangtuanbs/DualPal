# App icon sources

`app-icon.svg` is the current icon (split tile in orange tones with swap arrows). The
other SVGs are alternative designs.

To regenerate every platform icon after editing the SVG, render it to a
1024×1024 PNG at `src-tauri/icons/app-icon.png`, then run:

```sh
npx tauri icon src-tauri/icons/app-icon.png -o src-tauri/icons
```

(and delete the generated `android/` and `ios/` folders).
