# Keyboard and accessibility

What the interface does today, what is checked automatically, and what still needs a person.

## Keyboard

| Keys | Action |
| --- | --- |
| Ctrl+1 … Ctrl+5 | Overview, Models, Runtimes, Benchmarks, Lab |
| Ctrl+K | Go to Models and focus the search field. It never starts a search; Enter or Search does |
| Ctrl+R | Refresh the Overview or Runtimes page |

Every control is a standard GTK widget, so Tab order follows the visual order and Enter or Space activates it.
The Hugging Face token buttons sit in a wrapping flow box whose wrappers are not focusable, so each button is
one tab stop.

## Labels, colour, width

- Icon-only and ambiguous controls carry an accessible label or tooltip. State is always written as text
  (for example "Not measured"), never by colour alone.
- Pages fit a 360 px wide window; long text wraps and scrolls vertically instead of clipping.
- Sizes use GTK and Libadwaita defaults, so the desktop text-scaling setting applies. No custom animation is
  used beyond Libadwaita's, which follows the desktop reduced-motion setting.

## Automated check

`cargo test --locked -p workstation-app a11y_smoke -- --ignored` builds the Overview, Runtimes (with the attached
runtime cards), Models, Benchmarks and Lab pages and fails when a visible button, switch, spin button, search
field, drop-down or menu button has no label, tooltip, placeholder or mnemonic, or when a page needs more than
360 px. It needs a graphical display, so it is opt-in.

## Not checked automatically

A screen-reader pass (Orca), a pass in a real GNOME session at large text scale and with reduced motion, and a
contrast review of custom styles are manual. They are listed as pending validation in the beta checklist.
