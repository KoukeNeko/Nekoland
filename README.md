# Nekoland

A Wayland compositor written in Rust on [Smithay](https://github.com/Smithay/smithay).

It shows xdg-shell windows that can be moved and resized. Started inside another
Wayland or X11 session it runs as a window there; started on a bare console it
drives the display itself through DRM/KMS, with input from libinput.

On a bare console, Ctrl+Alt+Backspace quits.

## Build and run

```
cargo run -- -c foot
```

Without `-c`, it starts `foot`. Other clients connect through the
`WAYLAND_DISPLAY` it sets for the programs it starts.

## Origin

The starting point is `smallvil`, the minimal example compositor of Smithay
0.7.0, used under its MIT license (`licenses/SMITHAY-MIT.txt`).
