#!/usr/bin/env python3
"""Draws Nekoland's look for Qt programs, as a Kvantum theme, light and dark.

    make-theme.py OUTPUT_DIR

writes Nekoland.kvconfig, Nekoland.svg, NekolandDark.kvconfig and NekolandDark.svg into
OUTPUT_DIR, which is the theme's folder: Kvantum finds a dark theme in its light one's.

The colours are the GTK look's, in shell/gtk3/Nekoland/gtk-3.0/gtk.css and gtk-dark.css;
the shapes are those shell/gtk/nekoland.css gives GTK 4: filled buttons and fields on 8px
corners, what is chosen in solid ink. Kvantum draws a control from nine pieces of an
element, its four corners, four sides and middle, each a group of the SVG whose bounds are
the piece's; a state that is missing is drawn as the normal one.
"""

import sys
from pathlib import Path

LIGHT = {
    "paper": "#f4f3f0",
    "sheet": "#fbfaf8",
    "ink": "#17171a",
    "ink_2": "#5d5c61",
    "ink_3": "#6f6d72",
    "line": "#e2e0db",
    "fill": "#eae8e3",
    "on_ink": "#fbfaf8",
    "danger": "#c4372f",
}
DARK = {
    "paper": "#1b1b1e",
    "sheet": "#222226",
    "ink": "#f1efea",
    "ink_2": "#a9a7ab",
    "ink_3": "#939197",
    "line": "#323237",
    "fill": "#2c2c31",
    "on_ink": "#17171a",
    "danger": "#ff6b5f",
}

NONE = None
STATES = ("normal", "focused", "pressed", "toggled", "disabled")


def rgb(color):
    return [int(color[i : i + 2], 16) for i in (1, 3, 5)]


def mix(a, b, amount):
    """`a` with `amount` of `b`, as GTK's mix() does."""
    return "#" + "".join(f"{round(x + (y - x) * amount):02x}" for x, y in zip(rgb(a), rgb(b)))


def alpha(color, opacity):
    """`color` at `opacity`, as Kvantum writes it: #rrggbbaa."""
    return f"{color}{round(opacity * 255):02x}"


def paint(color):
    """An SVG fill for `color`, which may carry an alpha."""
    if len(color) == 9:
        return f'fill="{color[:7]}" fill-opacity="{int(color[7:], 16) / 255:.3f}"'
    return f'fill="{color}"'


class Sheet:
    """The SVG being drawn: every element is laid out in a column of its own."""

    def __init__(self):
        self.parts = []
        self.y = 0

    def group(self, name, x, y, w, h, shapes):
        # An invisible box gives the piece its bounds, whatever is drawn in it.
        body = [f'<rect x="{x}" y="{y}" width="{w}" height="{h}" fill="#000" fill-opacity="0"/>']
        body += shapes
        self.parts.append(f'<g id="{name}">' + "".join(body) + "</g>")

    def frame(self, name, fill, radius, size, border=NONE, width=1.0, sides="tblr"):
        """Nine pieces of a box filled with `fill`, its corners round by `radius`, inside
        a frame `size` wide, with a line of `border` along `sides`."""
        f, r, w = size, radius, width
        y, x = self.y, 0
        span = 20

        def corner(tx, ty, sx, sy, horizontal, vertical):
            # Drawn as the top left one and turned into place.
            shapes = []
            if fill:
                outline = f"M {f},{f} L 0,{f} L 0,{r} " + (f"A {r},{r} 0 0 1 {r},0 " if r else "") + f"L {f},0 Z"
                shapes.append(f'<path d="{outline}" {paint(fill)}/>')
            if border and (horizontal or vertical):
                if horizontal and vertical:
                    inner = max(r - w, 0)
                    ring = (
                        f"M 0,{f} L 0,{r} "
                        + (f"A {r},{r} 0 0 1 {r},0 " if r else "")
                        + f"L {f},0 L {f},{w} L {max(r, w)},{w} "
                        + (f"A {inner},{inner} 0 0 0 {w},{max(r, w)} " if inner else "")
                        + f"L {w},{f} Z"
                    )
                    shapes.append(f'<path d="{ring}" {paint(border)}/>')
                elif horizontal:
                    shapes.append(f'<rect x="0" y="0" width="{f}" height="{w}" {paint(border)}/>')
                else:
                    shapes.append(f'<rect x="0" y="0" width="{w}" height="{f}" {paint(border)}/>')
            body = "".join(shapes)
            return f'<g transform="translate({tx},{ty}) scale({sx},{sy})">{body}</g>'

        top, bottom, left, right = ("t" in sides, "b" in sides, "l" in sides, "r" in sides)
        pieces = {
            "topleft": (x, y, f, f, [corner(x, y, 1, 1, top, left)]),
            "topright": (x + f + span, y, f, f, [corner(x + 2 * f + span, y, -1, 1, top, right)]),
            "bottomleft": (x, y + f + span, f, f, [corner(x, y + 2 * f + span, 1, -1, bottom, left)]),
            "bottomright": (
                x + f + span,
                y + f + span,
                f,
                f,
                [corner(x + 2 * f + span, y + 2 * f + span, -1, -1, bottom, right)],
            ),
        }

        def side(px, py, pw, ph, line):
            shapes = []
            if fill:
                shapes.append(f'<rect x="{px}" y="{py}" width="{pw}" height="{ph}" {paint(fill)}/>')
            if border and line:
                lx, ly, lw, lh = line
                shapes.append(f'<rect x="{lx}" y="{ly}" width="{lw}" height="{lh}" {paint(border)}/>')
            return shapes

        pieces["top"] = (x + f, y, span, f, side(x + f, y, span, f, top and (x + f, y, span, w)))
        pieces["bottom"] = (
            x + f,
            y + f + span,
            span,
            f,
            side(x + f, y + f + span, span, f, bottom and (x + f, y + 2 * f + span - w, span, w)),
        )
        pieces["left"] = (x, y + f, f, span, side(x, y + f, f, span, left and (x, y + f, w, span)))
        pieces["right"] = (
            x + f + span,
            y + f,
            f,
            span,
            side(x + f + span, y + f, f, span, right and (x + 2 * f + span - w, y + f, w, span)),
        )
        for piece, (px, py, pw, ph, shapes) in pieces.items():
            self.group(f"{name}-{piece}", px, py, pw, ph, shapes)
        middle = [f'<rect x="{x + f}" y="{y + f}" width="{span}" height="{span}" {paint(fill)}/>'] if fill else []
        self.group(name, x + f, y + f, span, span, middle)
        self.y += 2 * f + span + 10

    def icon(self, name, size, shapes):
        """An indicator drawn in a `size` square from shapes in a 16 by 16 one."""
        y = self.y
        scaled = f'<g transform="translate(0,{y}) scale({size / 16})">' + "".join(shapes) + "</g>"
        self.group(name, 0, y, size, size, [scaled])
        self.y += size + 10


def stroke(color, d, width=1.5):
    if len(color) == 9:
        opacity = f' stroke-opacity="{int(color[7:], 16) / 255:.3f}"'
        color = color[:7]
    else:
        opacity = ""
    return (
        f'<path d="{d}" fill="none" stroke="{color}"{opacity} stroke-width="{width}" '
        'stroke-linecap="round" stroke-linejoin="round"/>'
    )


CHEVRONS = {
    "down": "M 4.5,6.5 L 8,10 L 11.5,6.5",
    "up": "M 4.5,9.5 L 8,6 L 11.5,9.5",
    "left": "M 9.5,4.5 L 6,8 L 9.5,11.5",
    "right": "M 6.5,4.5 L 10,8 L 6.5,11.5",
}


def draw(c):
    """The SVG for the palette `c`."""
    s = Sheet()
    hover = mix(c["fill"], c["ink"], 0.1)
    press = mix(c["fill"], c["ink"], 0.2)
    tint = alpha(c["ink"], 0.08)
    tint_press = alpha(c["ink"], 0.14)
    faint_fill = alpha(c["fill"], 0.5)

    # Buttons and the drop-downs, which GTK 4's look draws as buttons.
    for state, fill in zip(STATES, (c["fill"], hover, press, c["ink"], faint_fill)):
        s.frame(f"button-{state}", fill, 8, 8)
    # A drop-down is toggled while its list is open, and stays as pressed then.
    for state, fill in zip(STATES, (c["fill"], hover, press, press, faint_fill)):
        s.frame(f"combo-{state}", fill, 8, 8)
    # Flat buttons, on toolbars and as icons, take a tint only when pointed at.
    for state, fill in zip(STATES, (NONE, tint, tint_press, tint_press, NONE)):
        s.frame(f"tbutton-{state}", fill, 8, 8)
    # Fields are filled, and take an ink line when typed into.
    s.frame("lineedit-normal", c["fill"], 8, 8)
    s.frame("lineedit-focused", c["sheet"], 8, 8, c["ink"], 1.5)
    s.frame("lineedit-disabled", faint_fill, 8, 8)
    # The frame of a list or text view.
    s.frame("common-normal", NONE, 0, 1, c["line"])
    s.frame("common-focused", NONE, 0, 1, c["line"])
    s.frame("group-normal", NONE, 8, 8, c["line"])
    # Keyboard focus: a ring on the control's own corners, drawn over its frame; others
    # take the generic one, which Kvantum names without a state and draws at most 2px.
    focus = alpha(c["ink"], 0.5)
    for element in ("button", "combo", "tbutton", "group"):
        s.frame(f"{element}-focus", NONE, 8, 8, focus, 2)
    s.frame("tab-focus", NONE, 6, 6, focus, 2)
    s.frame("focus", NONE, 2, 2, focus, 1)
    # Tabs: the chosen one filled, on the paper of the page under them.
    s.frame("tabframe-normal", c["paper"], 8, 8, c["line"])
    for state, fill in zip(STATES, (NONE, tint, tint_press, c["fill"], NONE)):
        s.frame(f"tab-{state}", fill, 6, 6)
    # What is chosen in a list is solid ink.
    for state, fill in zip(STATES, (NONE, alpha(c["ink"], 0.06), c["ink"], c["ink"], NONE)):
        s.frame(f"itemview-{state}", fill, 6, 6)
    for state, fill in zip(STATES, (c["sheet"], mix(c["sheet"], c["ink"], 0.05), mix(c["sheet"], c["ink"], 0.1), c["sheet"], c["sheet"])):
        s.frame(f"header-{state}", fill, 0, 1, c["line"], 1, "b")
    s.icon("header-separator", 16, [f'<rect x="15" y="3" width="1" height="10" {paint(c["line"])}/>'])
    # Menus: a sheet, the entry pointed at solid ink on 6px corners.
    s.frame("menu-normal", c["sheet"], 8, 8, c["line"])
    for state, fill in zip(STATES, (NONE, c["ink"], c["ink"], c["ink"], NONE)):
        s.frame(f"menuitem-{state}", fill, 6, 6)
    s.icon("menuitem-separator", 16, [f'<rect x="0" y="7.5" width="16" height="1" {paint(c["line"])}/>'])
    for state, fill in zip(STATES, (NONE, tint, tint_press, tint_press, NONE)):
        s.frame(f"menubaritem-{state}", fill, 6, 6)
    s.frame("menubar-normal", NONE, 0, 1)
    s.frame("toolbar-normal", NONE, 0, 1)
    s.frame("dock-normal", NONE, 0, 1)
    s.frame("spin-normal", NONE, 0, 1)
    s.frame("titlebar-normal", c["paper"], 0, 1)
    s.frame("tooltip-normal", c["ink"], 8, 8)
    s.icon("toolbar-separator", 16, [f'<rect x="7.5" y="2" width="1" height="12" {paint(c["line"])}/>'])
    s.icon("toolbar-handle", 16, [])
    # Bars: a thin line of the line colour, the done part ink.
    s.frame("progress-normal", c["line"], 2, 2)
    s.frame("progress-pattern-normal", c["ink"], 2, 2)
    s.frame("progress-pattern-disabled", c["ink_3"], 2, 2)
    s.frame("slider-normal", c["line"], 2, 2)
    s.frame("slider-toggled", c["ink"], 2, 2)
    s.frame("slider-disabled", c["line"], 2, 2)
    s.frame("slider-toggled-disabled", c["ink_3"], 2, 2)
    handle = [
        f'<circle cx="8" cy="8" r="7.25" fill="{c["sheet"]}" stroke="{c["ink"]}" stroke-width="1.5"/>',
    ]
    for state in STATES:
        ring = c["ink_3"] if state == "disabled" else c["ink"]
        shape = [handle[0].replace(c["ink"], ring)]
        s.icon(f"slidercursor-{state}", 18, shape)
        s.icon(f"slidercursor-tickless-{state}", 18, shape)
    # Scrollbars: a pill of ink, thin until pointed at.
    for state, fill in zip(
        STATES, (alpha(c["ink"], 0.2), alpha(c["ink"], 0.4), alpha(c["ink"], 0.6), alpha(c["ink"], 0.4), alpha(c["ink"], 0.1))
    ):
        s.frame(f"scrollbarslider-{state}", fill, 4, 4)
    s.frame("scrollbargroove-normal", NONE, 0, 1)
    s.frame("splitter-normal", NONE, 0, 1)
    s.icon("splitter-grip-normal", 16, [])
    s.icon("grip-normal", 16, [])
    s.icon("resize-grip-normal", 16, [])
    s.icon("button-default-indicator", 16, [])
    s.icon("dial", 32, [f'<circle cx="8" cy="8" r="7.5" {paint(c["fill"])}/>'])
    s.icon("dial-handle", 16, [f'<circle cx="8" cy="8" r="4" {paint(c["ink"])}/>'])
    s.icon("dial-notches", 16, [])

    # Arrows, for buttons, drop-downs, menus and trees.
    for element in ("arrow", "carrow", "tarrow"):
        for way, d in CHEVRONS.items():
            for state in STATES:
                color = {"toggled": c["on_ink"], "disabled": c["ink_3"]}.get(state, c["ink"])
                s.icon(f"{element}-{way}-{state}", 16, [stroke(color, d)])
    # A drop-down's arrow is named without its way, which is down.
    for state in STATES:
        color = c["ink_3"] if state == "disabled" else c["ink"]
        s.icon(f"carrow-{state}", 16, [stroke(color, CHEVRONS["down"])])
    for way, d in (("right", CHEVRONS["right"]), ("left", CHEVRONS["left"])):
        for state in STATES:
            color = {"focused": c["on_ink"], "pressed": c["on_ink"], "toggled": c["on_ink"], "disabled": c["ink_3"]}.get(state, c["ink"])
            s.icon(f"menuitem-{way}-{state}", 16, [stroke(color, d)])
    for name, d in (("plus", CHEVRONS["right"]), ("minus", CHEVRONS["down"])):
        for state in STATES:
            color = c["ink_3"] if state == "disabled" else c["ink_2"]
            s.icon(f"tree-{name}-{state}", 16, [stroke(color, d)])
    # A number field's buttons.
    signs = {
        "plus": "M 4,8 H 12 M 8,4 V 12",
        "minus": "M 4,8 H 12",
        "up": CHEVRONS["up"],
        "down": CHEVRONS["down"],
    }
    for name, d in signs.items():
        for state in STATES:
            color = c["ink_3"] if state == "disabled" else c["ink"]
            back = {"focused": tint, "pressed": tint_press}.get(state)
            shapes = [f'<rect x="0" y="0" width="16" height="16" rx="4" {paint(back)}/>'] if back else []
            s.icon(f"spin-{name}-{state}", 16, shapes + [stroke(color, d)])
    for state in STATES:
        color = {"disabled": c["ink_3"], "toggled": c["ink"]}.get(state, c["ink_2"])
        back = {"focused": tint, "pressed": tint_press}.get(state)
        shapes = [f'<circle cx="8" cy="8" r="8" {paint(back)}/>'] if back else []
        s.icon(f"tab-close-{state}", 16, shapes + [stroke(color, "M 5,5 L 11,11 M 11,5 L 5,11")])

    # Checks and radios: an outline of the third ink, solid ink when on.
    def box(on, state, round_):
        disabled = state == "disabled"
        shape = '<circle cx="8" cy="8" r="{r}" {p}/>' if round_ else '<rect x="{o}" y="{o}" width="{w}" height="{w}" rx="{rx}" {p}/>'
        if not on:
            ring = c["ink_3"] if not disabled else alpha(c["ink_3"], 0.5)
            back = {"focused": tint, "pressed": tint_press}.get(state)
            parts = []
            if back:
                parts.append(shape.format(r=8, o=0, w=16, rx=5, p=paint(back)))
            outline = shape.format(r=7.25, o=0.75, w=14.5, rx=4.25, p='fill="none"')
            color, opacity = ring[:7], (f' stroke-opacity="{int(ring[7:], 16) / 255:.3f}"' if len(ring) == 9 else "")
            parts.append(outline.replace("/>", f' stroke="{color}"{opacity} stroke-width="1.5"/>'))
            return parts
        fill = c["ink_3"] if disabled else (mix(c["ink"], c["paper"], 0.15) if state == "focused" else c["ink"])
        return [shape.format(r=8, o=0, w=16, rx=5, p=paint(fill))]

    for state in STATES:
        s.icon(f"checkbox-{state}", 16, box(False, state, False))
        s.icon(
            f"checkbox-checked-{state}",
            16,
            box(True, state, False) + [stroke(c["on_ink"], "M 4.5,8.5 L 7,11 L 11.5,5.5", 1.75)],
        )
        s.icon(f"checkbox-tristate-{state}", 16, box(True, state, False) + [stroke(c["on_ink"], "M 5,8 H 11", 1.75)])
        s.icon(f"radio-{state}", 16, box(False, state, True))
        s.icon(f"radio-checked-{state}", 16, box(True, state, True) + [f'<circle cx="8" cy="8" r="3" {paint(c["on_ink"])}/>'])

    height = s.y
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="200" height="{height}" viewBox="0 0 200 {height}">\n'
        + "\n".join(s.parts)
        + "\n</svg>\n"
    )


def config(c, name):
    """The kvconfig for the palette `c`: what of each control the SVG draws, and its colours."""
    return f"""[%General]
comment=Nekoland's look
author=Nekoland
x11drag=menubar_and_primary_toolbar
alt_mnemonic=true
left_tabs=false
attach_active_tab=false
joined_inactive_tabs=false
group_toolbar_buttons=false
toolbar_item_spacing=2
toolbar_interior_spacing=4
spread_progressbar=true
composite=true
translucent_windows=false
blurring=false
popup_blurring=false
menu_shadow_depth=0
tooltip_shadow_depth=0
splitter_width=1
scroll_width=8
scroll_arrows=false
scroll_min_extent=40
transient_scrollbar=true
transient_groove=false
scrollbar_in_view=true
slider_width=4
slider_handle_width=18
slider_handle_length=18
tickless_slider_handle_size=18
check_size=16
textless_progressbar=false
progressbar_thickness=4
menubar_mouse_tracking=true
toolbutton_style=0
spin_button_width=24
vertical_spin_indicators=false
inline_spin_indicators=true
combo_as_lineedit=true
combo_menu=true
hide_combo_checkboxes=true
combo_focus_rect=false
groupbox_top_label=true
fill_rubberband=false
merge_menubar_with_toolbar=true
small_icon_size=16
large_icon_size=32
button_icon_size=16
toolbar_icon_size=16
layout_spacing=6
layout_margin=12
submenu_overlap=0
submenu_delay=250
tooltip_delay=-1
tree_branch_line=false
animate_states=false
no_window_pattern=true
respect_DE=false
reduce_window_opacity=0
click_behavior=0

[GeneralColors]
window.color={c["paper"]}
base.color={c["sheet"]}
alt.base.color={mix(c["sheet"], c["ink"], 0.03)}
button.color={c["fill"]}
light.color={c["sheet"]}
mid.light.color={c["fill"]}
dark.color={c["line"]}
mid.color={c["line"]}
highlight.color={c["ink"]}
inactive.highlight.color={c["ink"]}
text.color={c["ink"]}
window.text.color={c["ink"]}
button.text.color={c["ink"]}
disabled.text.color={c["ink_3"]}
tooltip.base.color={c["ink"]}
tooltip.text.color={c["on_ink"]}
highlight.text.color={c["on_ink"]}
link.color={c["ink"]}
link.visited.color={c["ink_2"]}
progress.indicator.text.color={c["ink"]}

[Hacks]
transparent_ktitle_label=true
transparent_dolphin_view=false
transparent_menutitle=true
respect_darkness=false
force_size_grip=false
iconless_pushbutton=false
iconless_menu=false
disabled_icon_opacity=50
normal_default_pushbutton=true
single_top_toolbar=true
tint_on_mouseover=0
no_selection_tint=true
transparent_arrow_button=true
middle_click_scroll=false
centered_forms=false
kinetic_scrolling=false
noninteger_translucency=false

[PanelButtonCommand]
frame=true
frame.element=button
frame.top=8
frame.bottom=8
frame.left=8
frame.right=8
interior=true
interior.element=button
indicator.element=arrow
indicator.size=16
text.normal.color={c["ink"]}
text.focus.color={c["ink"]}
text.press.color={c["ink"]}
text.toggle.color={c["on_ink"]}
text.disabled.color={c["ink_3"]}
text.shadow=false
text.bold=false
text.margin.top=1
text.margin.bottom=1
text.margin.left=4
text.margin.right=4
text.iconspacing=6
min_width=+3font
frame.expansion=0
focusFrame=true

[PanelButtonTool]
inherits=PanelButtonCommand
frame.element=tbutton
interior.element=tbutton
indicator.element=tarrow
text.toggle.color={c["ink"]}
min_width=0

[ToolbarButton]
frame.element=tbutton
interior.element=tbutton
indicator.element=tarrow
text.normal.color={c["ink"]}
text.focus.color={c["ink"]}
text.press.color={c["ink"]}
text.toggle.color={c["ink"]}

[ToolbarComboBox]
frame.element=combo
interior.element=combo
indicator.element=carrow
text.normal.color={c["ink"]}
text.focus.color={c["ink"]}
text.press.color={c["ink"]}
text.toggle.color={c["ink"]}

[ToolbarLineEdit]
frame.element=lineedit
interior.element=lineedit

[Dock]
inherits=PanelButtonCommand
frame.element=dock
interior.element=dock
frame.top=1
frame.bottom=1
frame.left=1
frame.right=1

[DockTitle]
inherits=PanelButtonCommand
frame=false
interior=false
text.bold=true

[IndicatorSpinBox]
inherits=PanelButtonCommand
frame=false
interior=false
frame.element=spin
interior.element=spin
indicator.element=spin
indicator.size=16
text.margin.top=0
text.margin.bottom=0
text.margin.left=0
text.margin.right=0

[RadioButton]
inherits=PanelButtonCommand
frame=false
interior.element=radio
text.margin.left=6
min_width=0

[CheckBox]
inherits=PanelButtonCommand
frame=false
interior.element=checkbox
text.margin.left=6
min_width=0

[Focus]
inherits=PanelButtonCommand
frame=true
interior=false
frame.element=focus
frame.top=2
frame.bottom=2
frame.left=2
frame.right=2

[GenericFrame]
inherits=PanelButtonCommand
frame=true
interior=false
frame.element=common
interior.element=common
frame.top=1
frame.bottom=1
frame.left=1
frame.right=1

[LineEdit]
inherits=PanelButtonCommand
frame.element=lineedit
interior.element=lineedit
text.margin.top=2
text.margin.bottom=2
text.margin.left=2
text.margin.right=2
min_width=0

[DropDownButton]
inherits=PanelButtonCommand
indicator.element=arrow

[IndicatorArrow]
indicator.element=arrow
indicator.size=16

[ToolboxTab]
inherits=PanelButtonCommand

[Tab]
inherits=PanelButtonCommand
frame.element=tab
interior.element=tab
indicator.element=tab
frame.top=6
frame.bottom=6
frame.left=6
frame.right=6
text.margin.top=1
text.margin.bottom=1
text.margin.left=8
text.margin.right=8
text.toggle.color={c["ink"]}
text.normal.color={c["ink_2"]}
text.focus.color={c["ink"]}
min_width=0

[TabFrame]
inherits=PanelButtonCommand
frame.element=tabframe
interior.element=tabframe
frame.top=8
frame.bottom=8
frame.left=8
frame.right=8

[TabBarFrame]
inherits=GenericFrame
frame=false
interior=false

[TreeExpander]
inherits=PanelButtonCommand
indicator.element=tree
indicator.size=16

[HeaderSection]
inherits=PanelButtonCommand
frame.element=header
interior.element=header
frame.top=1
frame.bottom=1
frame.left=1
frame.right=1
text.margin.top=3
text.margin.bottom=3
text.margin.left=6
text.margin.right=6
text.normal.color={c["ink_2"]}
text.focus.color={c["ink"]}
text.press.color={c["ink"]}
text.toggle.color={c["ink"]}
text.bold=true
min_width=0

[SizeGrip]
indicator.element=resize-grip

[Toolbar]
inherits=PanelButtonCommand
frame.element=toolbar
interior.element=toolbar
indicator.element=toolbar
indicator.size=16
frame.top=1
frame.bottom=1
frame.left=1
frame.right=1
text.margin=0
min_width=0

[Slider]
inherits=PanelButtonCommand
frame.element=slider
interior.element=slider
frame.top=2
frame.bottom=2
frame.left=2
frame.right=2

[SliderCursor]
inherits=PanelButtonCommand
frame=false
interior.element=slidercursor

[Progressbar]
inherits=PanelButtonCommand
frame.element=progress
interior.element=progress
frame.top=2
frame.bottom=2
frame.left=2
frame.right=2
text.margin=0
min_width=0

[ProgressbarContents]
inherits=PanelButtonCommand
frame=true
frame.element=progress-pattern
interior.element=progress-pattern
frame.top=2
frame.bottom=2
frame.left=2
frame.right=2

[ItemView]
inherits=PanelButtonCommand
frame.element=itemview
interior.element=itemview
frame.top=6
frame.bottom=6
frame.left=6
frame.right=6
text.margin.top=0
text.margin.bottom=0
text.margin.left=0
text.margin.right=0
text.normal.color={c["ink"]}
text.focus.color={c["ink"]}
text.press.color={c["on_ink"]}
text.toggle.color={c["on_ink"]}
min_width=0

[Splitter]
frame=false
interior=false
frame.element=splitter
interior.element=splitter
indicator.element=splitter-grip
indicator.size=16

[Scrollbar]
inherits=PanelButtonCommand
indicator.element=arrow
indicator.size=0

[ScrollbarGroove]
inherits=PanelButtonCommand
frame=false
interior=false
frame.element=scrollbargroove
interior.element=scrollbargroove

[ScrollbarSlider]
inherits=PanelButtonCommand
frame=true
interior=true
frame.element=scrollbarslider
interior.element=scrollbarslider
frame.top=4
frame.bottom=4
frame.left=4
frame.right=4
indicator.element=grip
indicator.size=0

[MenuItem]
inherits=PanelButtonCommand
frame=true
frame.element=menuitem
interior.element=menuitem
indicator.element=menuitem
frame.top=6
frame.bottom=6
frame.left=6
frame.right=6
text.margin.top=0
text.margin.bottom=0
text.margin.left=4
text.margin.right=4
text.normal.color={c["ink"]}
text.focus.color={c["on_ink"]}
text.press.color={c["on_ink"]}
text.toggle.color={c["on_ink"]}
min_width=0

[MenuBar]
inherits=PanelButtonCommand
frame.element=menubar
interior.element=menubar
frame.top=1
frame.bottom=1
frame.left=1
frame.right=1
min_width=0

[MenuBarItem]
inherits=PanelButtonCommand
frame.element=menubaritem
interior.element=menubaritem
frame.top=6
frame.bottom=6
frame.left=6
frame.right=6
text.margin.top=0
text.margin.bottom=0
text.margin.left=2
text.margin.right=2
text.toggle.color={c["ink"]}
min_width=0

[TitleBar]
inherits=PanelButtonCommand
frame=false
interior.element=titlebar
indicator.element=mdi
text.bold=true
min_width=0

[ComboBox]
inherits=PanelButtonCommand
frame.element=combo
interior.element=combo
indicator.element=carrow
text.toggle.color={c["ink"]}

[Menu]
inherits=PanelButtonCommand
frame.element=menu
interior.element=menu
frame.top=8
frame.bottom=8
frame.left=8
frame.right=8
text.normal.color={c["ink"]}
min_width=0

[GroupBox]
inherits=GenericFrame
frame=true
frame.element=group
interior=false
frame.top=8
frame.bottom=8
frame.left=8
frame.right=8
text.normal.color={c["ink"]}
text.focus.color={c["ink"]}
text.bold=true

[ToolTip]
inherits=GenericFrame
frame=true
interior=true
frame.element=tooltip
interior.element=tooltip
frame.top=8
frame.bottom=8
frame.left=8
frame.right=8
text.normal.color={c["on_ink"]}
text.focus.color={c["on_ink"]}
text.margin=0

[StatusBar]
inherits=GenericFrame
frame=false
interior=false

[Window]
interior=false
frame=false
"""


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    for name, palette in (("Nekoland", LIGHT), ("NekolandDark", DARK)):
        (out / f"{name}.svg").write_text(draw(palette))
        (out / f"{name}.kvconfig").write_text(config(palette, name))


if __name__ == "__main__":
    main()
