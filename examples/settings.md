# Settings

`jump` stores settings through **cosmic-config**, the same mechanism every
COSMIC application uses. They live under:

```
~/.config/cosmic/dev.entro314labs.Jump/v1/
```

One file per setting, each containing a RON value. Changes are picked up
**live** — the running daemon applies them without a restart.

```sh
# Use a centred panel instead of a full-screen Launchpad.
echo -n 'panel' > ~/.config/cosmic/dev.entro314labs.Jump/v1/grid_layout

# Turn compositor blur off.
echo -n 'false' > ~/.config/cosmic/dev.entro314labs.Jump/v1/blur
```

An unset key uses its default; an unreadable one logs a warning and falls back,
so a bad value can never stop the launcher from opening.

## Keys

| Key | Type | Default | Meaning |
|---|---|---|---|
| `scroll` | `continuous` \| `page-horizontal` \| `page-vertical` | `page-horizontal` | Launchpad paging |
| `blur` | bool | `true` | Compositor backdrop blur |
| `opacity` | float | `0.42` | Search panel fill; lower shows more desktop |
| `fullscreen_opacity` | float | `0.48` | Launchpad backdrop fill |
| `grid_layout` | `fullscreen` \| `panel` | `fullscreen` | Launchpad layout |
| `cell_size` | float | `158.0` | Target grid cell width; drives icon size |
| `grid_max_width` | float | `0.72` | Fraction of the display the grid may span |
| `disabled_plugins` | list of strings | `[]` | Plugin directory names switched off |
| `quicklinks` | list of links | `[]` | Keyworded URL templates, see below |
| `fallbacks` | list of links | DuckDuckGo, Wikipedia | Web searches appended below every search |
| `favorites` | list of strings | `[]` | Pinned result keys; written by the action panel's Pin on Top |
| `plugin_keywords` | list of `(id, keyword)` | `[]` | Alias a plugin's keyword without editing its manifest; `""` removes the keyword |
| `files` | struct | see below | File search |

### `quicklinks` and `fallbacks`

Both hold the same link shape; `{query}` in the template is replaced with the
percent-encoded query.

```ron
[
    (name: "YouTube", keyword: "yt", template: "https://www.youtube.com/results?search_query={query}"),
]
```

A **quicklink**'s keyword claims the query the way a plugin keyword does:
`yt cats` opens a YouTube search and nothing else answers. A **fallback** has
no keyword (the field is ignored); fallbacks are appended below the results of
every ordinary search, in configuration order, so a query that matched nothing
still ends somewhere useful. Set `fallbacks` to `[]` to turn the rows off.

### `files`

Written as a single RON struct, for example:

```ron
(
    enabled: true,
    external_drives: false,
    refresh_hours: 6,
    roots: [],
    ignore_names: [],
    ignore_paths: [],
    include_extensions: [],
    exclude_extensions: [],
    content: false,
    content_extensions: [],
    content_max_mb: 512,
    content_max_file_kb: 2048,
)
```

| Field | Meaning |
|---|---|
| `external_drives` | Index mounted NTFS/exFAT volumes. Off by default — these are routinely multi-terabyte. |
| `roots` | Extra trees to index, beyond `$HOME`. |
| `ignore_names` / `ignore_paths` | Applied at **index** time, so excluding a tree makes the index smaller *and* faster to build. |
| `include_extensions` / `exclude_extensions` | Applied at **query** time, since these change on a whim. |
| `content` | Index file *contents*, not just names. Off by default: extraction is expensive. |
| `content_max_mb` | Size ceiling for the content index. Indexing stops here, and because candidates are indexed in priority order the cap costs the least useful documents. |
| `content_max_file_kb` | Skip individual files larger than this. |
