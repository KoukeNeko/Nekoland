"""What Waydroid's clipboard bridge asks of pyclip, which Ubuntu has no package of: the
clipboard of the Wayland display Waydroid's session runs on, through wl-clipboard."""

import subprocess


def copy(text):
    if isinstance(text, str):
        text = text.encode()
    # wl-copy stays on to serve the clipboard; it keeps none of the caller's output open.
    subprocess.run(["wl-copy"], input=text, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)


def paste():
    # Android takes text only; with none on the clipboard wl-paste fails, and Waydroid
    # gives Android nothing.
    said = subprocess.run(["wl-paste", "--no-newline", "--type", "text"], capture_output=True, check=True)
    return said.stdout.decode(errors="replace")
