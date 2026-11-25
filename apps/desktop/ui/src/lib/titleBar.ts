/**
 * The title bar's leading cell (left of the bot chip): a drag region that keeps the chip centred
 * and is the mount point for a page-level Back control, so a view can put its Back button into
 * the header with `createPortal(button, document.getElementById(TITLEBAR_LEAD_ID))` instead of
 * floating one over the content.
 */
export const TITLEBAR_LEAD_ID = "titlebar-lead";
