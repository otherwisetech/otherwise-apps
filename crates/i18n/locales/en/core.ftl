# catalog
core.catalog.json = The catalog could not be read: {detail}
core.catalog.schema = The catalog uses format {n}, which this version doesn't understand. Update Otherwise Hub.
core.catalog.invalid = The catalog entry “{id}” is not valid ({why}).
# archives
core.archive.io = Could not unpack the download: {detail}
core.archive.format = The download is not a valid archive: {detail}
core.archive.unsafe = The download was refused: “{path}” would be written outside the app’s folder.
core.archive.unsupported = Unknown file type: {name}
# install
core.install.io = Could not write {path}: {detail}
core.install.no_payload = The download does not contain the app where expected.
core.install.wrong_bundle = The download contains a different app ({found} instead of {expected}); nothing was installed.
core.install.not_writable = No permission to install in {path}.
core.install.not_ours = {path} is not a place where Otherwise Hub installs this app; it was left alone.
core.install.trash = Could not move {path} to the Trash: {detail}
# network
core.net.rate_limited = GitHub is limiting requests right now; try again in a few minutes.
core.net.status = The server answered with error {code}.
core.net.transport = Network error: {detail}
core.net.io = Could not save the download: {detail}
core.net.too_large = The download is larger than expected.
core.net.cancelled = Cancelled.
core.net.size = The download is incomplete ({got} of {want} bytes).
# verification
core.verify.missing = GitHub has not published a SHA-256 checksum for this file, so it can’t be verified and was not installed.
core.verify.mismatch = The download does not match GitHub’s SHA-256 checksum (expected {expected}…, got {actual}…). It was deleted and nothing was installed.
core.net.not_http = Only http(s) addresses are supported.

# background jobs run by Otherwise Helper
core.background.sync = Refresh calendar subscriptions
core.background.upcoming = Upcoming events
core.background.synced.one = {n} subscription up to date
core.background.synced.other = {n} subscriptions up to date
core.background.sync_failed = {n} subscription(s) couldn't be refreshed
core.background.starts_at = Starts at {time}
core.background.next = Next: {title} · {when}
core.background.no_events = No events in the next 7 days
