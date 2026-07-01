---
name: nodriver
description: "Write Python browser automation scripts using nodriver (undetected Chrome DevTools Protocol). Use when: writing web scrapers, automating logins, filling forms, bypassing anti-bot systems (Cloudflare, Captcha), taking screenshots, managing cookies, or any browser automation that needs to avoid detection. Triggers on: nodriver, undetected browser, web scraping, browser automation, bypass captcha, bypass cloudflare, headless chrome, CDP automation."
metadata:
  {
    "openclaw":
      {
        "emoji": "🕸️",
        "requires": { "bins": ["python3"] },
        "install":
          [
            {
              "id": "pip",
              "kind": "pip",
              "package": "nodriver",
              "label": "Install nodriver (pip)",
            },
          ],
      },
  }
---

# nodriver

Async Python library for undetected browser automation via CDP. Successor to `undetected-chromedriver`. No Selenium, no chromedriver binary.

Docs: https://ultrafunkamsterdam.github.io/nodriver

## Prerequisites

- Chrome/Chromium/Edge/Brave installed (default location)
- Headless environments: use Xvfb or `headless=True`

## Quick start

```python
import nodriver as uc

async def main():
    browser = await uc.start()
    tab = await browser.get('https://example.com')

if __name__ == '__main__':
    uc.loop().run_until_complete(main())
```

## Critical quirks (non-obvious)

- **Never use `asyncio.run()`** — use `uc.loop().run_until_complete(main())`.
- **`await tab` between rapid interactions** — syncs DOM state, prevents stale refs. Insert whenever elements aren't found or interactions fail.
- **Fresh profile each run by default** — set `user_data_dir="/path"` to persist.
- **`find(text, best_match=True)`** is expensive but accurate (matches by text length). Use `select(css)` when selector is known.
- **All lookup methods retry until `timeout`** (default 10s) — double as wait conditions. `await tab.select('body')` = wait for page load.
- **`expert=True` increases detectability** — opens shadow roots and disables web security, but WAFs detect it more easily.
- **`verify_cf()` requires `opencv-python`** — English only, doesn't work in expert mode.

## Start options

```python
browser = await uc.start(
    headless=False,
    user_data_dir="/path/to/profile",
    browser_executable_path="/path/to/bin",
    browser_args=['--proxy-server=socks5://user:pass@host:port'],
    lang="en-US",
)
```

Or use `Config()` for the same options. Per-context proxies: `browser.create_context(proxy_server="socks5://...")`.

## Element lookup

```python
btn = await tab.find("Sign in", best_match=True)  # by text (smart match)
email = await tab.select("input[type=email]")      # by CSS selector
items = await tab.select_all("a[href]")            # all matching CSS
node = await tab.xpath("//div[@class='x']//p[1]")  # by XPath
all_btns = await tab.find_all("Add to cart")       # all matching text
```

## Cookies (persist logins)

```python
await browser.cookies.save('session.dat')
await browser.cookies.load('session.dat')
```

## Event handlers + custom CDP

```python
import nodriver.cdp as cdp
tab.add_handler(cdp.network.RequestWillBeSent, lambda e: print(e.request.url))
await tab.send(cdp.emulation.set_device_metrics_override(width=375, height=812, device_scale_factor=3, mobile=True))
```

## Anti-bot

```python
await tab.verify_cf()                            # Cloudflare checkbox (needs opencv-python)
await tab.template_location('template.png')      # custom image match in viewport
```

## Full API reference

For complete method signatures (Browser, Tab, Element, Config, CDP): [references/api.md](references/api.md)
