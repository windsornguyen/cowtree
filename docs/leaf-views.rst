Acquisition and synchronization
===============================

``Workspace::acquire`` reserves requested paths and existing descendants of a prefix.
It installs current clean origins before activation. Private edits are retained
when their original value still matches the granted origin. Divergent private
edits require explicit resolution. A request with no paths is invalid.

``Workspace::sync`` advances clean paths to the published tip. Dirty paths and every
path in an unresolved proposal remain untouched. Each path retains its own
origin, so a leaf can contain private edits based on older published values.
The Git view advances without overwriting working bytes. A caller-owned branch
or unexpected managed HEAD blocks the operation instead of being reset.

Both transitions retain an installation journal until the leaf record is durable.
Recovery retries an uncertain activation under its captured generation. A stale
generation rolls back the installation and releases only newly acquired authority.
Intervening edits block rollback and preserve the journal for inspection.

The caller excludes raw writers during capture and installation. The metadata
view records staged values; it is not a filesystem write interceptor. The mounted
profile exercises private edits, new paths, conflicts, sync, and a lost activation
reply through the real Rust process and real filesystem.
