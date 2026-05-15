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

function pageToMarkdownPath(page) {
  const normalized = page.replace(/^\/+/, '').replace(/\.md$/, '');
  return path.join(rootDir, `${normalized}.md`);
}

function pageUrl(page) {
  const normalized = page.replace(/^\/+/, '').replace(/\/index$/, '');
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

function ensureCleanDist() {
  fs.rmSync(distDir, { recursive: true, force: true });
  fs.mkdirSync(distDir, { recursive: true });
}

function copyIfExists(from, to) {
  if (!fs.existsSync(from)) return;
  fs.cpSync(from, to, { recursive: true });
}

function validatePage(page, markdownPath, rawContent) {
  if (!fs.existsSync(markdownPath)) {
    throw new Error(`Navigation page "${page}" does not exist at ${path.relative(rootDir, markdownPath)}`);
  }

  if (/\bStub\s+[—-]\s+fill in\b/.test(rawContent)) {
    throw new Error(`Navigation page "${page}" is still a scaffold stub`);
  }
}

function processPage(page) {
  const markdownPath = pageToMarkdownPath(page);
  const raw = fs.readFileSync(markdownPath, 'utf8');
  validatePage(page, markdownPath, raw);

  const { data, content } = matter(raw);
  const title = data.title || firstHeading(content) || 'Untitled';
  const description = data.description || data.summary || firstParagraph(content) || '';
  const html = md.render(content);

  return { page, title, description, html };
}

function renderPage(doc) {
  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>${escapeHtml(doc.title)} - ${escapeHtml(config.name)}</title>
  <meta name="description" content="${escapeHtml(doc.description)}">
  <link rel="icon" href="${escapeHtml(config.favicon)}">
  <link rel="stylesheet" href="/style.css">
</head>
<body>
  <main data-pagefind-body>
${doc.html}
  </main>
</body>
</html>`;
}

console.log(`Building ${config.name} documentation...`);

ensureCleanDist();
copyIfExists(path.join(rootDir, 'assets'), path.join(distDir, 'assets'));
copyIfExists(path.join(rootDir, 'style.css'), path.join(distDir, 'style.css'));
copyIfExists(path.join(rootDir, 'nav-tabs-underline.js'), path.join(distDir, 'nav-tabs-underline.js'));

const pages = uniquePages();
console.log(`Found ${pages.length} navigation pages`);

let processedCount = 0;
for (const page of pages) {
  const doc = processPage(page);
  const outPath = outputPathForPage(page);
  fs.mkdirSync(path.dirname(outPath), { recursive: true });
  fs.writeFileSync(outPath, renderPage(doc));
  processedCount++;
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

console.log(`Built ${processedCount} public pages`);
console.log(`Output: ${distDir}`);
