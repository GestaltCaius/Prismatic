# 0004. Nexus Mods is used like a browser, with the user's session cookie

Status: accepted

## Decision
Nexus Mods has no API for a basic account, so Prismatic downloads the way a signed-in browser does: with the
user's `nexusmods_session` cookie, reading the site's pages for the file to fetch, and presenting Chrome's TLS and
HTTP/2 fingerprint so Cloudflare lets the requests through. That is why the HTTP client is one that can emulate a
browser, built on BoringSSL.

The cookie comes from, in this order: the one pasted in the page, the `NEXUS_COOKIE` variable, the browser's
cookie store. Prismatic itself never writes it to disk, never logs it, and never puts it in an exported mod list.
The page keeps a pasted cookie in the browser's own storage so it need not be pasted again. Types that hold it
print a redacted form. At most two downloads run at once, more gets the
account rate-limited.

## Forbidden
- Writing the cookie to a file on disk, a log or an export.
- A `Debug` or `Display` form of anything holding the cookie that shows it.
- Raising the download limit without checking how Nexus reacts.

## Revisit when
Nexus offers an API a basic account can use for downloads.

## Cost if wrong
Nexus or Cloudflare changes its pages or its checks and downloads break until the code follows. The session
cookie is a password, so a leak would let someone act as the user on Nexus.
