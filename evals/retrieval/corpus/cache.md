# Tenant cache namespaces

## invalidateTenantCache

The PHP helper invalidateTenantCache changes a tenant's cache namespace version
after its settings transaction commits. Each key includes both the tenant ID and
the namespace version. This avoids scanning every cache key during invalidation
and prevents one tenant's page fragment from being returned to another tenant.
The cache version is read from trusted application state, not from a request
parameter provided by a browser.

```php
function invalidateTenantCache(int $tenantId): void {
    CacheNamespace::increment($tenantId);
}

function tenantSettingsKey(int $tenantId, int $version): string {
    return "tenant:{$tenantId}:v{$version}:settings";
}
```

## Lifetime and race behavior

Old namespace entries expire naturally. A worker that began before invalidation
may finish computing a value for the old version, but it cannot publish that
value into the new version. Capture the version before starting work and use the
same captured value when writing. Never read a new namespace version only after
performing expensive work against old settings.

## Cache failure

A cache outage falls back to the authoritative database for small reads. The
caller limits concurrency so many identical requests do not overload the
database simultaneously. Mutating operations do not report success before their
database transaction commits. Cache invalidation failure is recorded for repair,
while the committed state remains authoritative.

This namespace technique does not authorize access to a tenant. Access checks
run before either cache lookup or database lookup. A cache key that is difficult
to guess is not an authorization policy. Tests use two tenants with identical
record numbers and distinct values to expose accidentally omitted tenant scopes.
