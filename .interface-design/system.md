# Shorui design system

Offline, all-in-one PDF toolbox for desktop. Built in Rust with GPUI and gpuikit, so everything here
must be drawable with rectangles, borders, text and images. The design canvas still shows the earlier working name "Foldo".

## Direction and feel

- **Who:** someone with a stack of PDFs between them and their real work. Keyboard-led, often batching many files.
- **Verb:** queue files, set a few options, run, confirm the output.
- **Feel:** a prepress desk at night. Quiet, dense, precise. The app is graphite; page images are the only bright surface, so the documents carry the hierarchy.
- **Dark is the primary theme.** Light is a full alternate with the same tokens.
- **School, not copy:** dense, dark-first, hairline-bordered, keyboard-first. No other product's accent, layouts or components are reused.

### Signature

1. **Paper is the light.** `stock` is the only bright fill: page thumbnails, the canvas page, the Home drop-zone sheet stack, the logo mark, signature cards.
2. **The run line.** The bottom bar reads left to right as input, arrow, output path, estimate, one action:
   `4 files · 58 pages → ~/Documents/Merged/Q3-report-merged.pdf · est. 12.0 MB  [Merge 4 files  Cmd Enter]`
3. **Mono for anything measured or typed:** sizes, page counts, page ranges, paths, shortcuts.

### Rejected defaults

- A colour per tool group as fills → one accent; group identity is a 6px dot beside the group name.
- A grid of tool cards with icon tiles → five bordered group lists of plain 32px rows.
- Stacked form in the options panel → inspector rows, label left and control right.
- Card-per-file queues → flat rows with column heads and hairline separators.

## Depth

**Borders only.** Hairline rules plus small lightness steps. No shadows, no gradients, no blur.
Overlays separate with `sleeve` fill, a `rule-strong` edge and a flat `scrim` behind the palette.
The window is `frame`. The sidebar sits straight on it, with no divider line. Everything else (top bar, workspace, options panel, run line) sits on one content card: `card` fill, a `rule` edge, radius 12, inset 8px from the top, right and bottom of the window. Inside the card the regions are divided by `rule`. The close button's hover follows the card's top-right curve.

## Spacing

Base unit 4px. Scale: 4, 8, 12, 16, 24, 32. Layout gutters use 16; 8px grid for anything structural.

## Radius

`r-xs` 4 (checkbox, key cap, chip) · `r-sm` 6 (buttons, inputs, rows) · `r-md` 8 (popover, toast, group list) · `r-lg` 12 (palette, dialog, drop zone).
Paper is 2px. Switch track, radio mark and slider thumb are fully round.

## Type

IBM Plex Sans (400, 500, 600) and IBM Plex Mono (400, 500). Both SIL OFL, bundleable. Base 13px, line height 1.4.

| Token | Spec | Use |
| --- | --- | --- |
| type.display | 18 / 600 / -0.01em | Home drop zone |
| type.heading | 13 / 600 | Mode name, panel and list titles |
| type.label | 13 / 500 | Buttons, nav items, file names |
| type.body | 13 / 400 | Inspector labels, prose |
| type.section | 12 / 500, toner-3 | Section labels, sentence case, never all caps |
| type.small | 12 / 400 | Helper text, column heads |
| type.mono | 12 / 400 mono | Sizes, counts, ranges, paths |
| type.mono-sm | 11 / 400 mono | Key caps, tiny counts |

## Colour tokens

| Token | Dark | Light | Role |
| --- | --- | --- | --- |
| bed | #101113 | #F6F6F4 | App base, sidebar, panels |
| frame | #0B0C0E | #F6F6F4 | The window: behind the sidebar and around the content card |
| card | #101113 | #FDFDFC | The content card |
| plate | #18191C | #EDEDEA | Hover fill, default button |
| plate-2 | #25272C | #E4E4E0 | Selected row, active nav item |
| plate-3 | #2F3238 | #DADAD5 | Pressed, tracks, tooltip |
| sleeve | #1A1B1F | #FFFFFF | Popover, palette, dialog, toast |
| well | #0A0B0C | #ECECE8 | Inputs, page canvas, drop zone |
| toner | #ECEDEE | #1B1C1E | Primary text |
| toner-2 | #A4A7AD | #4A4D54 | Labels, secondary text |
| toner-3 | #868991 | #63666E | Meta, column heads, section labels |
| toner-4 | #55585F | #A0A3A9 | Disabled, placeholder |
| rule-soft | white 5% | black 6% | Row separators |
| rule | white 8% | black 9% | Panel edges |
| rule-strong | white 14% | black 16% | Overlay edges, dashed drop zone |
| control-rule | white 12% | black 14% | Input and button borders |
| cyan | #4CC2DE | #0A7690 | Primary action, focus ring, selection |
| cyan-hi / cyan-lo | #6FD0E8 / #38AECB | #0C86A3 / #08667D | Primary hover / pressed |
| cyan-ink | #06222B | #FFFFFF | Text on cyan |
| cyan-wash / cyan-rule | cyan 14% / 45% | cyan 12% / 45% | Selected fill / its edge |
| proof | #4CB782 | #1F8A5B | Done, "Local only" dot |
| flag | #D9A63A | #9A6B00 | Warning |
| stamp | #F0736A | #B93228 | Error text and icon |
| stamp-fill | #C4372D | #C4372D | Destructive button, white text |
| stamp-wash / stamp-rule | red 12% / 38% | red 8% / 32% | Permanent-action notice |
| stock | #F4F2EC | #FFFFFF | Paper |
| blackout | #0A0A0A | #0A0A0A | Redaction mark |
| g-organise | #8C96F0 | #6C78E8 | Group dot |
| g-convert | #DDAE4A | #C99A2E | Group dot |
| g-edit | #5CC8A0 | #2FA87C | Group dot |
| g-secure | #E77C6E | #D9614F | Group dot |
| g-optimise | #B99AEC | #9B78DD | Group dot |

Colour rules: grey builds structure, cyan means "act, focused or selected", red means irreversible or failed.
Group colours never fill anything larger than a dot.

## States

- **Hover:** one plate up (`transparent → plate`, `plate → plate-2`). Primary goes to `cyan-hi`.
- **Pressed:** two plates up, primary to `cyan-lo`.
- **Focus:** 2px `cyan` ring with a 1px gap, on every interactive element. Focused list rows use an inset ring plus `plate`.
- **Selected:** `cyan-wash` fill with a `cyan-rule` edge. Active nav item uses `plate-2`, no accent.
- **Disabled:** `toner-4` text, `rule-soft` border, no fill change beyond `plate`. Locked panels drop to 45% opacity.

## Shell

| Region | Size | Notes |
| --- | --- | --- |
| Card inset | 8px | Gap between the content card and the window's top, right and bottom edges. |
| Sidebar | 220px, rail 48px | Home, Pinned, Recent, then Tools as five collapsible groups. Only the active group is open. Settings and theme toggle at the bottom. |
| Top bar | 44px | Group dot, group name, chevron, mode name; open file chips; search field with Cmd K; "Local only" badge. |
| Workspace | flexible | One of: flat queue, page thumbnail grid, single-page canvas. |
| Options panel | 300px | Inspector. 44px header with Reset. |
| Run line | 52px | Input summary, arrow, output path button, estimate, then buttons. One primary. |

Minimum window 1100×700: sidebar becomes the rail, file chips collapse to a count, search collapses to an icon with key caps.

## Component patterns

- **Button:** heights 24 / 28 / 32, radius 6, 13/500, padding 0 10. Variants: primary (cyan), default (plate + control-rule), ghost (transparent, toner-2), destructive (stamp-fill). Primary is 32 high and lives only in the run line and dialogs; it carries its shortcut as key caps.
- **Key cap:** 18px high, mono 11, `key` fill with `rule` edge. On a primary button: black 16% fill, no edge.
- **Nav item:** 28px, radius 6, 14px icon in toner-3, label 13/500 toner-2. Active: `plate-2`, toner text and icon.
- **Group header:** 28px, 10px chevron, 6px group dot, name 12/500, count in mono toner-4.
- **Tool row:** 32px, radius 6, icon then name; shortcut key caps appear on hover.
- **Queue row:** 48 to 52px, padding 0 16, `rule-soft` separator. Order: handle or checkbox, index, 22×30 paper thumb, name 13/500, right-aligned mono columns, inline control, remove button. 28px column head row above.
- **Drag:** lifted row or page gets `plate-2` and a `rule-strong` edge; the drop target is a 2px cyan line.
- **Page cell:** 124×172 paper in a 5px padded button; label below in mono 11. Selected adds cyan-wash, cyan-rule and a 16px cyan check badge.
- **Inspector section:** padding 8 16 12, `rule-soft` top edge, section label 12/500 toner-3. Property row 32px, label toner-2 left, control right. Long text inputs stack under their label.
- **Input:** 28px, `well` fill, control-rule edge, radius 6. Mono 12 for ranges and file names.
- **Select:** 28px button on `plate` with a 12px chevron.
- **Checkbox:** 14px, radius 4; checked is cyan with a cyan-ink tick.
- **Switch:** 26×16 track, 12px knob; on is cyan.
- **Slider:** 4px track on plate-3, cyan fill, 12px round thumb.
- **Segmented tabs:** `well` container with 2px padding; active segment `plate-2`.
- **Toggle chip:** 24px, radius 6; on is cyan-wash with cyan-rule, optional mono count.
- **Progress:** 4px bar on plate-3, cyan fill, proof when complete.
- **Command palette:** 640px, radius 12, `sleeve` on `scrim`. 48px input, 36px rows, selected row `plate-2`, matched letters in cyan 600, group shown as dot plus name, 36px hint footer.
- **Popover and toast:** `sleeve`, `rule-strong` edge, radius 8. Toast is 340px with a status icon, title 13/500 and one line of detail.
- **Dialog:** 400px, radius 12, buttons right-aligned, destructive confirm in stamp-fill.
- **Permanent-action notice:** stamp-wash fill, stamp-rule edge, alert icon in stamp, 13/600 title, 12px body.
- **Error row:** alert icon in stamp, a "Failed" chip, a plain-language reason in toner-2, then the fix as a default button.
- **Canvas:** `well` stage, 44px toolbar with page stepper and zoom. Floating tool strip and pickers are `sleeve` with radius 8; the active tool is cyan-wash with a cyan icon.
- **Editing on the page (Edit & Fill):** nothing is typed in the options panel. The 44px bar above the page holds the tools as 26px icon buttons in groups split by 16px rules (select; text, date; tick, cross, dot; circle, line, highlight, white out), then a size stepper, three ink swatches and Undo. A click on the page types in place; a click on a form field fills it where it sits; a click on a tick box toggles it. Fillable fields carry a faint blue wash. The selected item gets a 1px `stock-select` outline 3px outside it and a 16px remove handle on its corner. Typed text shows in the system face that matches Helvetica so it lands where it will be saved. The panel only reports what is on the page, lists the keys, and holds the save options.
- **Icons:** 1.5px stroke, round caps and joins, 14px in controls.

## Open items for GPUI

- The drop zone uses a dashed border.
- Switches, radio marks and slider thumbs are fully rounded.
- Scrim and locked-panel dimming rely on alpha fills.
