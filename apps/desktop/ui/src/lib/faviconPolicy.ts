/**
 * A favicon is fetched from the network only for a site the owner already allowed (a standing
 * grant listed in Settings, or a navigation they approved). The host of a pending approval card
 * is chosen by the model, so it must never trigger a lookup: the daemon would resolve and request
 * a name an injected prompt can fill with data. Pending cards show the neutral letter / globe tile.
 */
export function mayFetchFavicon(allowed: boolean | undefined, domain: string): boolean {
  return allowed === true && /^[a-z0-9.-]+\.[a-z]{2,}$/i.test(domain.trim());
}

/**
 * The domain whose real icon an activity step chip may request, or null for the letter / globe.
 * A chip names the host the model asked for, so it stays neutral until the step finished fine
 * (`done`): that is when the owner approved the navigation or a standing grant covered it, i.e. the
 * origin was visited in this run. Requested, awaiting-approval, running, denied, expired and failed
 * steps never fetch. Only the host parsed from the step's own tool input is ever passed in, never
 * a URL the page supplied.
 */
export function chipFaviconDomain(status: string, host: string | null | undefined): string | null {
  if (status !== "done" || !host) return null;
  const domain = host.trim().toLowerCase();
  return mayFetchFavicon(true, domain) ? domain : null;
}
