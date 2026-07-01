# nodriver API Reference

Full docs: https://ultrafunkamsterdam.github.io/nodriver

## Table of Contents
- [Module-level](#module-level)
- [Browser](#browser)
- [Tab](#tab)
- [Element](#element)
- [Config](#config)
- [CDP Usage](#cdp-usage)

---

## Module-level

```python
import nodriver as uc

browser = await uc.start(**kwargs)  # → Browser (see Config params below)
loop = uc.loop()                    # → asyncio event loop
```

## Browser

Created via `await uc.start()` or `await Browser.create(config)`.

### Properties
| Property | Type | Description |
|----------|------|-------------|
| `.tabs` | `List[Tab]` | Open page-type targets |
| `.main_tab` | `Tab` | Tab launched with browser |
| `.cookies` | `CookieJar` | Cookie manager |
| `.targets` | `List` | All targets (pages, workers, etc.) |
| `.websocket_url` | `str` | Debug WebSocket URL |
| `.stopped` | `bool` | Whether browser is stopped |

### Methods
| Method | Returns | Description |
|--------|---------|-------------|
| `await .get(url, new_tab=False, new_window=False)` | `Tab` | Navigate or open tab/window |
| `await .create_context(url, proxy_server=None, ...)` | `Tab` | New browser context (isolated proxy) |
| `await .wait(time=0.1)` / `.sleep(time)` | — | Wait seconds |
| `await .grant_all_permissions()` | — | Grant all browser permissions |
| `await .tile_windows(windows, max_columns)` | — | Tile open windows |
| `.stop()` | — | Kill browser process |

### CookieJar
| Method | Description |
|--------|-------------|
| `await .save(filepath='.session.dat')` | Save cookies to file |
| `await .load(filepath='.session.dat')` | Load cookies from file |
| `await .get_all(requests_cookie_format=False)` | Get all cookies |

## Tab

Represents a page, window, iframe, or background target.

### Key Methods

#### Navigation
| Method | Description |
|--------|-------------|
| `await .get(url)` | Navigate to URL |
| `await .back()` | History back |
| `await .forward()` | History forward |
| `await .reload()` | Reload page |
| `await .close()` | Close tab |
| `await .activate()` / `.bring_to_front()` | Focus tab |

#### Element Lookup
| Method | Returns | Description |
|--------|---------|-------------|
| `await .find(text, best_match=True, timeout=10)` | `Element` | Find by text (smart match) |
| `await .find_all(text, timeout=10)` | `List[Element]` | Find all by text |
| `await .select(css_selector, timeout=10)` | `Element` | Find by CSS selector |
| `await .select_all(css_selector, timeout=10)` | `List[Element]` | Find all by CSS selector |
| `await .xpath(selector, timeout=2.5)` | `Element` | Find by XPath |
| `await .query_selector_all(selector)` | `List[Element]` | Low-level query (no retry) |
| `await .find_elements_by_text(text)` | `List[Element]` | Low-level text search (no retry) |

#### Page Interaction
| Method | Description |
|--------|-------------|
| `await .evaluate(expression, await_promise=False)` | Execute JS, return result |
| `await .get_content()` | Get page HTML |
| `await .save_screenshot(path)` | Save screenshot |
| `await .scroll_down(amount)` / `.scroll_up(amount)` | Scroll pixels |
| `await .sleep(seconds)` | Wait |
| `await tab` | Sync DOM / "breathe" |

#### Storage & Security
| Method | Description |
|--------|-------------|
| `await .get_local_storage()` | Get localStorage dict |
| `await .set_local_storage(dict)` | Set localStorage |
| `await .bypass_insecure_connection_warning()` | Accept invalid cert |
| `await .open_external_debugger()` | Open DevTools |

#### Anti-Bot
| Method | Description |
|--------|-------------|
| `await .verify_cf()` | Click Cloudflare checkbox (needs opencv-python) |
| `await .template_location(img_path)` | Find template image position in viewport |

#### Events
| Method | Description |
|--------|-------------|
| `.add_handler(event_type, callback)` | Register CDP event handler |
| `await .send(cdp_command)` | Send raw CDP command |

#### Downloads
| Method | Description |
|--------|-------------|
| `await .download_file(url, filename=None)` | Download file by URL |

## Element

Returned by `tab.find()`, `tab.select()`, etc.

### Properties
| Property | Type | Description |
|----------|------|-------------|
| `.text` | `str` | Text content |
| `.text_all` | `str` | All text including children |
| `.tag` / `.tag_name` | `str` | HTML tag name |
| `.attrs` | `dict` | HTML attributes |
| `.node_id` | `int` | DOM node ID |
| `.backend_node_id` | `int` | Backend node ID |
| `.children` | `List[Element]\|str` | Child elements |
| `.parent` | `Element\|None` | Parent element |
| `.shadow_children` | `List[Element]` | Shadow DOM children |
| `.tab` | `Tab` | Owning tab |
| `.value` | `str` | Input value |
| `.remote_object` | `RemoteObject` | JS remote object |
| `.object_id` | `RemoteObjectId` | JS object ID |

### Methods
| Method | Description |
|--------|-------------|
| `await .click()` | JS-level click (preferred) |
| `await .mouse_click(button='left', modifiers=0)` | Native mouse click |
| `await .send_keys(text)` | Type text into element |
| `await .clear_input()` | Clear input field |
| `await .apply(js_function)` | Apply JS function to element |
| `await .get_js_attributes()` | Get JS properties |
| `await .get_position(abs=False)` | Get x,y position |
| `await .scroll_into_view()` | Scroll element into view |
| `await .flash(duration=0.5)` | Flash element (debug) |
| `await .highlight_overlay()` | Highlight with overlay |
| `await .record_video(path, duration)` | Record element video |
| `await .save_to_dom()` | Persist modifications |
| `await .remove_from_dom()` | Remove from DOM |
| `await .update()` | Refresh element data |
| `await .set_value(value)` | Set input value directly |
| `await .set_text(value)` | Set text content |

## Config

```python
from nodriver import Config

config = Config()
config.headless = False
config.user_data_dir = "/path/to/profile"
config.browser_executable_path = "/path/to/chrome"
config.browser_args = ['--flag=value']
config.lang = "en-US"
config.add_argument('--proxy-server=...')
```

Pass to `start()`: `browser = await uc.start(config=config)`

Or pass kwargs directly: `browser = await uc.start(headless=True, user_data_dir="...")`

## CDP Usage

All Chrome DevTools Protocol domains available under `nodriver.cdp`:

```python
import nodriver.cdp as cdp

# Send any CDP command
await tab.send(cdp.page.navigate(url='https://...'))
await tab.send(cdp.emulation.set_device_metrics_override(
    width=375, height=812, device_scale_factor=3, mobile=True
))

# Common domains: page, network, dom, runtime, emulation, input_, security, storage
```

Event handler pattern:
```python
tab.add_handler(cdp.network.RequestWillBeSent, lambda e: print(e.request.url))
```

Full CDP reference: https://chromedevtools.github.io/devtools-protocol/
