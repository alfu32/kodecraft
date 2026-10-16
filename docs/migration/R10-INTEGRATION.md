# R10 integration contract

This document records the implementation design for the active goal, not
acceptance evidence or a completion claim. R09 owns protocol signatures; R10
owns token acquisition and its UI/session lifecycle.

## OAuth service boundary

Use oauth2 5.0.0 with default transport features disabled. Its selected API/source
has been inspected: PKCE authorization URLs, code/client-credentials/refresh
exchange and device authorization/polling. Adapt the workspace reqwest 0.13 client
to the library's HTTP request/response trait; do not pull in a second reqwest
version merely for the library's optional transport. Redirects remain disabled
for credential-bearing endpoint requests and token bodies have a bounded limit.
The operation's cancellation/deadline covers network, callback and device waits.

Expose non-serializable configuration captures, progress and token results.
Progress can contain a public authorization URL/device code, never client secrets,
PKCE verifier, access/refresh tokens or raw token endpoint responses. Redact error
and Debug output. Use OAuth 1.0's existing signer for temporary credential and
access-token endpoints, adding callback/verifier parameters through its public
builder. Validate token confirmation, matching temporary token and callback data.

## Callback and browser flow

Authorization-code acquisition generates fresh state and S256 PKCE, binds only
loopback when receiving a callback locally, and displays the authorization URL.
B008 defaults to actual HTTPS at `https://localhost:<free-port>/callback` with a
per-flow in-memory self-signed certificate. The form explains browser trust and
provider callback registration. Postman callback defaults resolve to this local
listener without mutating the imported collection. HTTP loopback remains optional.
The user completes login in a GUI browser; no emulated provider login form is
added to Kode. An explicit pasted full redirect handles remote sessions where a
GUI browser cannot directly reach the IDE's loopback port. Validate callback
origin/path, duplicate parameters, state/code and OAuth error responses before
exchanging the code. Keep callback reads bounded. Wrong-state callbacks must not
consume an otherwise valid pending flow. Cancel releases listeners immediately.

Device flow displays verification URL and user code and uses provider interval,
pending/slow-down handling and expiry under the same cancellable owner. Refresh
is an explicit action and preserves the previous usable token if refresh fails.
Never use password or implicit grants as a substitute for browser acquisition.

## Captured request ownership

Keep flow jobs and token results keyed by workspace generation and the existing
runtime REST item identity, separate from imported/persisted collection IDs and
node paths. Rebase via existing stable runtime keys; discard results after item
removal, collection replacement or workspace disposal. Switching visible requests
cannot attach another request's token. Capture resolved endpoint/client settings
and environment so later edits cannot silently alter an in-flight flow.

The request form provides Acquire, Cancel, Copy URL, Paste redirect and explicit
Refresh/Clear actions, with visible pending/error/success/expiry status. It keeps
normal typing and request switching responsive. Reuse existing clipboard jobs and
modal/readonly editor controls for selectable URLs and redirect entry; callbacks
must satisfy the same proof/epoch checks as other asynchronous input.

Acquired tokens stay outside collection/session/export serialization. Sending an
individual request or running a collection captures runtime token overrides by
stable item identity before pure resolution; missing/expired tokens fail clearly
and never acquire or refresh implicitly. Authorization previews identify dynamic
runtime authentication without exposing newly acquired tokens automatically.

## Qualification

Use synthetic local identity providers and real browser/PTY sessions. Cover PKCE,
callback/state/owner rejection, public/confidential client authentication,
client-credentials, expiry, refresh rotation/failure, device pending/slow-down/
expiry/cancel and OAuth 1.0 temporary/access credential exchange. Complete an
actual browser login followed by an authorized request; verify export/restart has
no acquired tokens and shutdown leaves no listener or owned job. R10 remains
unfinished until implementation and these checks exist. The implementation and
acceptance results are recorded in [R10 evidence](../parity/R10.md).
