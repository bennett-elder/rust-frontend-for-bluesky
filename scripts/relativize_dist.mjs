// Rewrite the bundle's asset URLs to be relative.
//
// dx emits root-absolute paths in two places, and both break a project page
// (`https://user.github.io/<repo>/`):
//
//   index.html   href/src="/./assets/<bundle>.js"
//   the JS       module_or_path:"/./assets/<wasm>"   ← the one that actually bites
//
// The second is not visible in the HTML at all: the page loads, then the bootstrap asks for
// `/assets/<wasm>` at the *domain root* and gets a 404, leaving a blank page. Relative URLs
// resolve against the document, so one artifact works at `/<repo>/` and at a domain root.
//
// Usage: node scripts/relativize_dist.mjs [index.html]
import { copyFileSync, existsSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';

const file = process.argv[2] ?? 'target/dx/bluesky-alt-first-app/release/web/public/index.html';
const dist = dirname(file);
const html = readFileSync(file, 'utf8');
let rewritten = html.replace(/(href|src)="\/\.?\//g, '$1="./');
writeFileSync(file, rewritten);

// The deployment's name, when the build was given one. dx writes the static title from
// Dioxus.toml, which cannot know what this deploy calls itself.
const name = process.env.APP_NAME?.trim();
if (name) {
  rewritten = rewritten.replace(/<title>[^<]*<\/title>/, `<title>${name.replace(/[<>&]/g, '')}</title>`);
  writeFileSync(file, rewritten);
}

// dx preloads the bundle with `crossorigin`, which makes the hint a *different* fetch from the
// same-origin module import it is meant to warm: the browser then warns that the preloaded
// resource was never used. Same origin, so the attribute is noise.
rewritten = rewritten.replace(/(<link rel="preload"[^>]*?) crossorigin(?=[ >])/g, '$1');
writeFileSync(file, rewritten);

// The site icon. dx has no place for one in `index.html` (it writes that file itself), so the
// icon lives with the app's assets and is copied in here, with a *relative* link — the same
// reasoning as every other URL in this file. Only when the source exists, so a build with no
// icon gets no broken reference.
const iconSource = join('app', 'assets', 'favicon.ico');
if (existsSync(iconSource)) {
  copyFileSync(iconSource, join(dist, 'favicon.ico'));
  if (!rewritten.includes('rel="icon"')) {
    rewritten = rewritten.replace(
      '</title>',
      `</title>
    <link rel="icon" href="./favicon.ico">`,
    );
    writeFileSync(file, rewritten);
  }
}

// The bundles' own references to the asset directory.
let bundles = 0;
for (const entry of readdirSync(join(dist, 'assets'))) {
  if (!entry.endsWith('.js')) continue;
  const path = join(dist, 'assets', entry);
  const source = readFileSync(path, 'utf8');
  const fixed = source
    .replace(/(["'`])\/\.\/assets\//g, '$1./assets/')
    .replace(/(["'`])\/assets\//g, '$1./assets/');
  if (fixed !== source) {
    writeFileSync(path, fixed);
    bundles += 1;
  }
}

const absolute = [...rewritten.matchAll(/(?:href|src)="\/[^"]*"/g)].map((match) => match[0]);
const absoluteInBundle = readdirSync(join(dist, 'assets'))
  .filter((entry) => entry.endsWith('.js'))
  .flatMap((entry) => {
    const source = readFileSync(join(dist, 'assets', entry), 'utf8');
    return [...source.matchAll(/["'`]\/assets\/[^"'`]*/g)].map((match) => `${entry}: ${match[0]}`);
  });
if (absolute.length || absoluteInBundle.length) {
  console.error('relativize: absolute asset URLs remain:', { html: absolute, bundles: absoluteInBundle.slice(0, 4) });
  process.exit(1);
}
console.log(
  `relativize: ok (${absolute.length === 0 ? 'relative' : 'rewrote'} ${file}${name ? `, title → ${name}` : ''}; ${bundles} bundle(s) rewritten)`,
);
