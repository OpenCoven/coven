#!/usr/bin/env node

/**
 * Build the curated Coven documentation site.
 *
 * The public site intentionally follows docs.json navigation instead of
 * rendering every markdown file in this repository. The repo still contains
 * planning notes, maintenance docs, and scaffolded stubs that should not appear
 * in the public app or search index.
 */

import fs from 'fs';
import path from 'path';
import matter from 'gray-matter';
import MarkdownIt from 'markdown-it';
import anchor from 'markdown-it-anchor';

const md = new MarkdownIt({ html: true, linkify: true, typographer: false });
md.use(anchor);

const rootDir = process.cwd();
const distDir = path.join(rootDir, 'dist', 'docs-site');
const configPath = path.join(rootDir, 'docs.json');
const config = JSON.parse(fs.readFileSync(configPath, 'utf8'));

function collectPages(node, pages = []) {
  if (Array.isArray(node)) {
    for (const item of node) collectPages(item, pages);
    return pages;
  }

  if (!node || typeof node !== 'object') return pages;

  if (Array.isArray(node.pages)) {
    for (const page of node.pages) {
      if (typeof page === 'string') pages.push(page);
      else collectPages(page, pages);
    }
  }

  for (const value of Object.values(node)) {
    if (value !== node.pages) collectPages(value, pages);
  }

  return pages;
}

function uniquePages() {
  return [...new Set(collectPages(config.navigation ?? {}))];
}

function flattenNavigation() {
  const entries = [];

  for (const language of config.navigation?.languages ?? []) {
    for (const tab of language.tabs ?? []) {
      for (const group of tab.groups ?? []) {
        for (const page of group.pages ?? []) {
          if (typeof page !== 'string') continue;
          entries.push({
            language: language.language,
            tab: tab.tab,
            group: group.group,
            page,
            url: pageUrl(page)
          });
        }
      }
    }
  }

  return entries;
}

function pageToMarkdownPath(page) {
  const normalized = page.replace(/^\/+/, '').replace(/\.md$/, '');
  return path.join(rootDir, `${normalized}.md`);
}

function pageUrl(page) {
  const normalized = page.replace(/^\/+/, '').replace(/\.md$/, '').replace(/\/index$/, '');
  return normalized === 'index' ? '/' : `/${normalized}`;
}

function outputPathForPage(page) {
  const normalized = page.replace(/^\/+/, '').replace(/\.md$/, '');
  if (normalized === 'index') return path.join(distDir, 'index.html');
  return path.join(distDir, normalized, 'index.html');
}

function firstHeading(markdown) {
  const match = markdown.match(/^#\s+(.+)$/m);
  return match?.[1]?.trim();
}

function firstParagraph(markdown) {
  return markdown
    .split(/\n{2,}/)
    .map((chunk) => chunk.trim())
    .find((chunk) => chunk && !chunk.startsWith('#') && !chunk.startsWith('<'))
    ?.replace(/\s+/g, ' ')
    .slice(0, 180);
}

function escapeHtml(value) {
  return String(value)
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;');
}

function escapeAttr(value) {
  return escapeHtml(value).replaceAll("'", '&#39;');
}

function ensureCleanDist() {
  fs.rmSync(distDir, { recursive: true, force: true });
  fs.mkdirSync(distDir, { recursive: true });
}

function copyIfExists(from, to) {
  if (!fs.existsSync(from)) return;
  fs.cpSync(from, to, { recursive: true });
}

function copyPublicAssets() {
  const assetsDir = path.join(distDir, 'assets');
  fs.mkdirSync(assetsDir, { recursive: true });
  fs.copyFileSync(
    path.join(rootDir, 'assets', 'opencoven-icon.svg'),
    path.join(assetsDir, 'opencoven-icon.svg')
  );
}

function validatePage(page, markdownPath, rawContent) {
  if (!fs.existsSync(markdownPath)) {
    throw new Error(`Navigation page "${page}" does not exist at ${path.relative(rootDir, markdownPath)}`);
  }

  const { data } = matter(rawContent);
  for (const field of ['title', 'summary', 'read_when']) {
    if (!data[field] || (field === 'read_when' && (!Array.isArray(data[field]) || data[field].length === 0))) {
      throw new Error(`Navigation page "${page}" is missing frontmatter field "${field}"`);
    }
  }

  if (/\bStub\s+[—-]\s+fill in\b/.test(rawContent)) {
    throw new Error(`Navigation page "${page}" is still a scaffold stub`);
  }

  const publicPlaceholder = /\b(Image asset prompt|lorem ipsum|FIXME|coming soon|TBD)\b/i;
  if (publicPlaceholder.test(rawContent)) {
    throw new Error(`Navigation page "${page}" contains placeholder or unsituated copy`);
  }
}

function readAttrs(rawAttrs) {
  const attrs = {};
  const attrPattern = /([A-Za-z0-9_-]+)="([^"]*)"/g;
  let match;

  while ((match = attrPattern.exec(rawAttrs))) {
    attrs[match[1]] = match[2];
  }

  return attrs;
}

function dedent(markdown) {
  const lines = markdown.replace(/^\n+|\n+$/g, '').split('\n');
  const indents = lines
    .filter((line) => line.trim())
    .map((line) => line.match(/^ */)?.[0].length ?? 0);
  const minIndent = indents.length ? Math.min(...indents) : 0;

  if (minIndent <= 0) {
    return lines.join('\n').trim();
  }

  return lines.map((line) => line.slice(minIndent)).join('\n').trim();
}

function renderInnerMarkdown(markdown) {
  return md.render(renderMintlifyBlocks(dedent(markdown)));
}

function renderCards(markdown) {
  return markdown.replace(/<Columns>(?:\r?\n)?([\s\S]*?)(?:\r?\n)?<\/Columns>/g, (_match, body) => {
    const cards = body.replace(/<Card\b([^>]*)>(?:\r?\n)?([\s\S]*?)(?:\r?\n)?<\/Card>/g, (_card, rawAttrs, cardBody) => {
      const attrs = readAttrs(rawAttrs);
      const title = escapeHtml(attrs.title || 'Untitled');
      const content = renderInnerMarkdown(cardBody);

      if (attrs.href) {
        return `<a class="doc-card" href="${escapeAttr(attrs.href)}"><span class="doc-card-title">${title}</span><div class="doc-card-copy">${content}</div></a>`;
      }

      return `<section class="doc-card"><span class="doc-card-title">${title}</span><div class="doc-card-copy">${content}</div></section>`;
    });

    return `<div class="doc-grid">${cards}</div>`;
  });
}

function renderSteps(markdown) {
  return markdown.replace(/<Steps>(?:\r?\n)?([\s\S]*?)(?:\r?\n)?<\/Steps>/g, (_match, body) => {
    const steps = body.replace(/<Step\b([^>]*)>(?:\r?\n)?([\s\S]*?)(?:\r?\n)?<\/Step>/g, (_step, rawAttrs, stepBody) => {
      const attrs = readAttrs(rawAttrs);
      const title = escapeHtml(attrs.title || 'Step');
      return `<li class="doc-step"><div class="doc-step-body"><h3 data-toc="false">${title}</h3>${renderInnerMarkdown(stepBody)}</div></li>`;
    });

    return `<ol class="doc-steps">${steps}</ol>`;
  });
}

function renderTabs(markdown) {
  return markdown.replace(/<Tabs>(?:\r?\n)?([\s\S]*?)(?:\r?\n)?<\/Tabs>/g, (_match, body) => {
    const tabs = body.replace(/<Tab\b([^>]*)>(?:\r?\n)?([\s\S]*?)(?:\r?\n)?<\/Tab>/g, (_tab, rawAttrs, tabBody) => {
      const attrs = readAttrs(rawAttrs);
      const title = escapeHtml(attrs.title || 'Tab');
      return `<section class="doc-tab"><h3 data-toc="false">${title}</h3>${renderInnerMarkdown(tabBody)}</section>`;
    });

    return `<div class="doc-tabs">${tabs}</div>`;
  });
}

function renderCallouts(markdown) {
  return markdown.replace(/<(Tip|Note|Info|Warning|Frame)>(?:\r?\n)?([\s\S]*?)(?:\r?\n)?<\/\1>/g, (_match, kind, body) => {
    return `<aside class="doc-callout doc-callout-${kind.toLowerCase()}">${renderInnerMarkdown(body)}</aside>`;
  });
}

function renderMintlifyBlocks(markdown) {
  let rendered = markdown;
  rendered = renderCards(rendered);
  rendered = renderSteps(rendered);
  rendered = renderTabs(rendered);
  rendered = renderCallouts(rendered);
  return rendered;
}

function decodeHtml(value) {
  return String(value)
    .replaceAll('&amp;', '&')
    .replaceAll('&lt;', '<')
    .replaceAll('&gt;', '>')
    .replaceAll('&quot;', '"')
    .replaceAll('&#39;', "'");
}

function stripTags(value) {
  return decodeHtml(String(value))
    .replace(/[<>]/g, '')
    .replace(/\s+/g, ' ')
    .trim();
}

function collectToc(html) {
  const toc = [];
  const headingPattern = /<h([23])\b([^>]*)>([\s\S]*?)<\/h\1>/g;
  let match;

  while ((match = headingPattern.exec(html))) {
    const level = Number(match[1]);
    const attrs = readAttrs(match[2]);
    if (!attrs.id || attrs['data-toc'] === 'false') continue;

    const title = stripTags(match[3]);
    if (!title) continue;

    toc.push({
      id: attrs.id,
      level,
      title
    });
  }

  return toc;
}

function processPage(page) {
  const markdownPath = pageToMarkdownPath(page);
  const raw = fs.readFileSync(markdownPath, 'utf8');
  validatePage(page, markdownPath, raw);

  const { data, content } = matter(raw);
  const title = data.title || firstHeading(content) || 'Untitled';
  const description = data.description || data.summary || firstParagraph(content) || '';
  const html = md.render(renderMintlifyBlocks(content));
  const toc = collectToc(html);

  return { page, title, description, html, toc, url: pageUrl(page), hasH1: /^#\s+.+$/m.test(content) };
}

function renderNavList(entries, currentUrl) {
  let previousLanguage = '';
  let previousTab = '';
  let previousGroup = '';
  let html = '<nav class="docs-nav" aria-label="Documentation">';

  for (const entry of entries) {
    if (entry.language !== previousLanguage) {
      if (previousGroup) html += '</ul>';
      if (previousTab) html += '</section>';
      if (previousLanguage) html += '</section>';
      html += `<section class="docs-nav-language"><h2>${escapeHtml(entry.language.toUpperCase())}</h2>`;
      previousLanguage = entry.language;
      previousTab = '';
      previousGroup = '';
    }

    if (entry.tab !== previousTab) {
      if (previousGroup) html += '</ul>';
      if (previousTab) html += '</section>';
      html += `<section class="docs-nav-tab"><h3>${escapeHtml(entry.tab)}</h3>`;
      previousTab = entry.tab;
      previousGroup = '';
    }

    if (entry.group !== previousGroup) {
      if (previousGroup) html += '</ul>';
      html += `<p>${escapeHtml(entry.group)}</p><ul>`;
      previousGroup = entry.group;
    }

    const active = entry.url === currentUrl ? ' aria-current="page"' : '';
    html += `<li><a href="${escapeAttr(entry.url)}"${active}>${escapeHtml(entry.title)}</a></li>`;
  }

  if (previousGroup) html += '</ul>';
  if (previousTab) html += '</section>';
  if (previousLanguage) html += '</section>';
  html += '</nav>';

  return html;
}

function renderTopLinks() {
  return (config.navbar?.links ?? [])
    .map((link) => `<a href="${escapeAttr(link.href)}">${escapeHtml(link.label)}</a>`)
    .join('');
}

function renderPageNav(doc, entries) {
  const index = entries.findIndex((entry) => entry.page === doc.page);
  const previous = index > 0 ? entries[index - 1] : null;
  const next = index >= 0 && index < entries.length - 1 ? entries[index + 1] : null;

  if (!previous && !next) return '';

  return `<nav class="page-nav" aria-label="Previous and next pages">
    ${previous ? `<a href="${escapeAttr(previous.url)}"><span>Previous</span>${escapeHtml(previous.title)}</a>` : '<span></span>'}
    ${next ? `<a href="${escapeAttr(next.url)}"><span>Next</span>${escapeHtml(next.title)}</a>` : '<span></span>'}
  </nav>`;
}

function renderToc(toc) {
  if (!toc.length) return '';

  return `<aside class="toc-sidebar" aria-label="On this page">
    <p>On this page</p>
    <nav>
      ${toc
        .map(
          (item) =>
            `<a class="toc-link toc-link-level-${item.level}" href="#${escapeAttr(item.id)}">${escapeHtml(item.title)}</a>`
        )
        .join('')}
    </nav>
  </aside>`;
}

function languageForPage(page) {
  if (page.startsWith('es/')) return 'es';
  if (page.startsWith('ru/')) return 'ru';
  return 'en';
}

function renderPage(doc, entries) {
  const nav = renderNavList(entries, doc.url);
  return `<!doctype html>
<html lang="${escapeAttr(languageForPage(doc.page))}">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>${escapeHtml(doc.title)} - ${escapeHtml(config.name)}</title>
  <meta name="description" content="${escapeHtml(doc.description)}">
  <link rel="icon" href="${escapeHtml(config.favicon)}">
  <link rel="stylesheet" href="/style.css">
  <script src="/sidebar-nav.js" defer></script>
</head>
<body${doc.toc.length ? ' class="has-toc"' : ''}>
  <header class="site-header">
    <div class="header-left">
      <button class="sidebar-trigger" type="button" aria-label="Toggle docs navigation" aria-controls="docs-sidebar" aria-expanded="true" data-sidebar-trigger>
        <span class="sidebar-trigger-icon" aria-hidden="true"><span></span><span></span><span></span></span>
      </button>
      <a class="brand-link" href="/">
        <img src="/assets/opencoven-icon.svg" alt="" width="32" height="32">
        <span>${escapeHtml(config.name)} Docs</span>
      </a>
    </div>
    <nav class="site-links" aria-label="Project links">
      ${renderTopLinks()}
    </nav>
  </header>
  <div class="docs-layout">
    <div class="sidebar-overlay" data-sidebar-overlay hidden></div>
    <aside class="sidebar" id="docs-sidebar" aria-label="Documentation navigation">
      <div class="sidebar-panel-header">
        <span>Docs</span>
        <button class="sidebar-close" type="button" aria-label="Close docs navigation" data-sidebar-close>Close</button>
      </div>
      ${nav}
    </aside>
    <main class="doc-content" data-pagefind-body>
      ${doc.hasH1 ? '' : `<h1>${escapeHtml(doc.title)}</h1>`}
${doc.html}
      ${renderPageNav(doc, entries)}
    </main>
    ${renderToc(doc.toc)}
  </div>
</body>
</html>`;
}

console.log(`Building ${config.name} documentation...`);

ensureCleanDist();
copyPublicAssets();
copyIfExists(path.join(rootDir, 'style.css'), path.join(distDir, 'style.css'));
copyIfExists(path.join(rootDir, 'nav-tabs-underline.js'), path.join(distDir, 'nav-tabs-underline.js'));
copyIfExists(path.join(rootDir, 'sidebar-nav.js'), path.join(distDir, 'sidebar-nav.js'));

const pages = uniquePages();
console.log(`Found ${pages.length} navigation pages`);

const docs = pages.map(processPage);
const docByPage = new Map(docs.map((doc) => [doc.page, doc]));
const navEntries = flattenNavigation().map((entry) => ({
  ...entry,
  title: docByPage.get(entry.page)?.title || entry.page
}));

for (const page of pages) {
  const doc = docByPage.get(page);
  const outPath = outputPathForPage(page);
  fs.mkdirSync(path.dirname(outPath), { recursive: true });
  fs.writeFileSync(outPath, renderPage(doc, navEntries));
}

fs.writeFileSync(
  path.join(distDir, 'manifest.json'),
  JSON.stringify(
    {
      name: config.name,
      description: config.description,
      pages: pages.map((page) => ({ page, url: pageUrl(page) }))
    },
    null,
    2
  )
);

console.log(`Built ${docs.length} public pages`);
console.log(`Output: ${distDir}`);
