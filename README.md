# 🦎 Monitor Lizard

The monitor settings Windows forgot about. Tiny, free, no background process.

Open it, change things, close it. Nothing keeps running.

## Status

**v0.1 backend (command line).** The UI comes next.

## Use it

```
monitor-lizard list
monitor-lizard set 1 --brightness 40
monitor-lizard set all --brightness 70 --contrast 60
monitor-lizard set 2 --input hdmi1
monitor-lizard inputs
```

## Build

```
cargo build --release
```

The app ends up in `target\release\monitor-lizard.exe`.

## Notes

- Works with external monitors over **DDC/CI**. Turn DDC/CI on in your monitor's menu if it's off.
- Some docks and USB-C hubs (especially DisplayLink) block DDC/CI.
- Laptop built-in screens: brightness works (through Windows WMI). Contrast and input are external-monitor only.

## License

MIT
