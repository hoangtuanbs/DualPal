# App icon sources

`app-icon.svg` is the current icon (two glossy panes, amber on the left and red on the right, with swap arrows on a dark tile). The
other SVGs are alternative designs.

To regenerate every platform icon after editing the SVG, render it to a
1024×1024 PNG at `src-tauri/icons/app-icon.png`, then run:

```sh
npx tauri icon src-tauri/icons/app-icon.png -o src-tauri/icons
```

(and delete the generated `android/` and `ios/` folders).

`app-icon-small.svg` is a simplified version (thicker arrows, wider gap, no
gloss) used for the 16–32 px sizes in `icon.ico`, `32x32.png`,
`Square30x30Logo.png` and `Square44x44Logo.png`, which are built from both
renders with Pillow so that small sizes stay sharp.
