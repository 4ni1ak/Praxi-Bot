# Theme accessibility

## Four palettes

The UI fixture supports light, dark, midnight and warm palettes. Components use
semantic tokens for foreground, muted text, surfaces, borders and focus rings.
The warm palette uses amber for keyboard focus. Its focus outline must remain
visible against both the elevated panel and the surrounding application canvas.
Do not reuse a muted border token for the active focus indicator.

## Contrast review

Measure theme contrast using the actual computed foreground/background pairs.
Review disabled controls separately from explanatory text; text describing why a
control is unavailable still needs to be readable. Status messages include an
icon or short label, because color alone is insufficient to distinguish warning,
success and failure. Keep the same semantic meaning in all four palettes.

| Element | Keyboard behavior | Palette requirement |
| --- | --- | --- |
| Source card | Tab enters the primary action | Visible amber focus in warm |
| Dialog | Escape closes after cancellation | Backdrop stays distinct |
| Switch | Space toggles | Text identifies enabled state |
| Error message | Linked from affected field | Readable contrast |

## Interaction checks

Open every settings dialog with the keyboard, move through controls in document
order, close it and verify focus returns to the opener. A loading state must not
replace the focused button with a different inaccessible element. Long translated
labels wrap within the card rather than expanding the page width. Screenshots
are useful for visual inspection, but assertions on computed color pairs catch
regressions that a screenshot reviewer can overlook.

Animations respect the operating system's reduced-motion preference. This does
not disable progress text: an indexing operation still reports its current state
and allows cancellation even when animated spinners are hidden.
