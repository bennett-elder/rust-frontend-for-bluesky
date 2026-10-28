# Rusty — an alt-text-first Bluesky client

An alt-text-first Bluesky client: **alt text is the post's media**, and an image or video is
opt-in per embed. It reads everything public — feeds, threads, profiles, search, trending —
and writes only what a reader needs to take part: posts and replies, likes, reposts, and
muting or blocking an account.

Rust with [Dioxus](https://dioxuslabs.com), as a web app (WASM) and an Android WebView app
from one codebase.

## Using it

Nothing is fetched until you scroll, and nothing plays until you ask.

- **Embeds start as their description.** Every image and video renders as alt text first: what
the poster said it shows. `ALT` reveals the media, `×` hides it again, video plays on request.
An embed with no description is marked *No Alt Text* rather than quietly showing a picture —
the whole point is to make that visible.
- **Feeds** — Explore (search and trending), What's Hot, Following, and your saved feeds, each
on its own tab, with endless scroll and a Back that returns to where you were.
- **Threads** open in place with ancestors above the focal post; replies nest, and more than 20
direct replies fold behind a disclosure you can open.
- **Post menus** — copy the post's link (this app, or bsky.app) or its text; mute or block the
author, from the profile or the post itself.
- **New post** — text only, from the rail or the tab row. Posting lands on your own profile,
where the post is; a feed shows it once its revalidation runs.
- **Light, dark, or system**, remembered per device.
- Real links throughout: cmd-click opens a new tab, and `#/post/<at-uri>` is a shareable deep
link.

## Sign in

Optional. Two paths, and a deployment decides which it offers:

- **App password** — paste one from [Bluesky settings](https://bsky.app/settings/app-passwords).
It is exchanged for a session and never stored. Memory-only unless you tick *remember me*.
- **Bluesky OAuth** — approve the request on Bluesky and come back signed in. Every request is
then signed by a key that stays in this browser and cannot be exported, so a stolen token
alone is useless. Kept across reloads unless you opt out, and signing out revokes it at the
server.

**Settings** says which kind of session you hold, and whether it survives a reload.

What the app writes, in full: a new post, a reply to an existing post, a like, a repost, a
block, a mute — and the deletes that undo them. It does not follow anyone or delete posts.
A new post is text only for now: images would mean requiring alt text. URLs, mentions and
hashtags in a post or a reply are annotated, so they are clickable for everyone who reads it.

## Run it locally

```bash
cd app && ../.tools/dx build --release --platform web && cd ..
node scripts/relativize_dist.mjs
python3 scripts/serve_web.py 8099 target/dx/bluesky-alt-first-app/release/web/public
# → http://127.0.0.1:8099/
```

`serve_web.py` sends `no-store` on purpose: dx content-hashes the bundle but not `index.html`,
and an ordinary static server will happily hand you an older build.

Android is a milestone, not a target: `cd app && ../.tools/dx build --platform android`. It
lags the web build.

## Deploy your own (GitHub Pages)

`.github/workflows/deploy-pages.yml` builds and publishes on every push to `main`.

**A fork** — fork the repo, then *Settings → Pages → Source: **GitHub Actions***. Nothing else:
no variables, no base path, no source edits. It publishes to
`https://<you>.github.io/<repo>/`.

**Your own domain** — *Settings → Pages → Custom domain*, point `CNAME` at
`<you>.github.io`, tick *Enforce HTTPS*, then set the repository variable
**`SITE_URL=https://your.domain/`**. A domain cannot be derived from a repository name, and the
OAuth client metadata has to name the origin it is served from.

## Configuration

All four are build inputs, read at compile time; CI feeds the last three from repository
variables (*Settings → Secrets and variables → Actions → Variables*).

| Variable | Effect | Default |
| --- | --- | --- |
| `APP_NAME` | Name in the rail, the header and every tab title | `Rusty` |
| `REPO_URL` | Where "fork for yourself" points | this repository |
| `OAUTH_MODE` | `off`, `both` or `oauth` — which sign-in paths the UI offers | `off` |
| `SITE_URL` | The deployed origin, for the OAuth client metadata | derived from the repo |

`OAUTH_MODE` is the one worth reading twice. `off` gives app passwords only; `both` puts both
options on the sign-in page; `oauth` makes OAuth the primary action. Anything unrecognised
falls back to `off`, so a fork that sets nothing can never end up with a sign-in page that
does not work.

OAuth needs one file, `oauth-client-metadata.json`, whose `client_id` is its own URL — atproto
has no app registry, so that URL *is* the client identity. The workflow generates it. After
deploying, check it against the URL it is served from:

```bash
node scripts/check_client_metadata.mjs https://your.domain/
```

A mismatch is otherwise only visible as an opaque error at sign-in.

## Checks

```bash
cargo test -p alt-first-core              # rendering policy, OAuth primitives, fixtures
node scripts/check_pages_subpath.mjs      # boots at /<repo>/ and at a domain root
node scripts/check_client_metadata.mjs <site-url>
```

`scripts/` also holds browser checks driven over the DevTools protocol — alt-first embeds,
pagination, scroll memory, themes, contrast, menus, reply context, and the OAuth start and
callback paths. Each takes the base URL it should drive.

## License

MIT — see [`LICENSE`](LICENSE), and the same field in both crate manifests.
