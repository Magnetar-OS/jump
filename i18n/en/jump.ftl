app-title = Jump
app-description = Search applications, windows, files, and clipboard

# The query field's placeholder.
type-to-search = Search

# A quicklink or fallback web-search row. { $name } is the site ("DuckDuckGo"),
# { $query } is what the user typed.
web-search-for = Search { $name } for “{ $query }”
web-search-subtitle = Web search

# Subtitle of an emoji result; Enter copies the emoji.
emoji-copy-subtitle = Copy to clipboard

## Status-area menu
tray-search = Search…
tray-applications = Applications
tray-settings = Settings…
tray-rebuild-index = Rebuild file index
tray-clear-clipboard = Clear clipboard history
tray-quit = Quit

## Settings window
settings-title = Jump Settings

section-appearance = Appearance
background-blur = Background blur
panel-opacity = Search panel opacity
backdrop-opacity = Launchpad backdrop opacity

section-launchpad = Launchpad
layout = Layout
layout-fullscreen = Full screen
layout-panel = Centred panel
scrolling = Scrolling
scroll-continuous = Continuous
scroll-pages-horizontal = Pages, horizontal
scroll-pages-vertical = Pages, vertical
icon-size = Icon size
grid-width = Grid width

section-files = File search
files-by-name = Search files by name
files-external-drives = Include external drives
files-content = Search inside files
files-content-limit = Content index limit (MB)
files-refresh-hours = Rebuild index every (hours)

note-live = Changes apply immediately; the launcher does not need restarting.
note-unavailable = Settings cannot be saved: cosmic-config is unavailable.

## Built-in system commands
system-dark-mode = Toggle Dark Mode
system-dark-mode-subtitle = Switch the desktop between light and dark
system-settings-subtitle = COSMIC Settings
system-page-displays = Displays Settings
system-page-appearance = Appearance Settings
system-page-wallpaper = Wallpaper Settings
system-page-network = Network Settings
system-page-bluetooth = Bluetooth Settings
system-page-sound = Sound Settings
system-page-power = Power Settings
system-page-keyboard = Keyboard Settings
system-page-mouse = Mouse & Touchpad Settings
system-page-users = User Accounts
system-page-datetime = Date & Time Settings

## Settings window: plugins
section-plugins = Plugins
plugins-none = No plugins installed.
plugins-hint = Plugins live in ~/.local/share/jump/plugins — see the example in the repository.

## Action panel
action-open = Open
action-open-folder = Open Enclosing Folder
action-copy-path = Copy Path
action-copy = Copy
action-copy-link = Copy Link
action-pin = Pin on Top
action-unpin = Unpin
action-maximize-window = Maximize
action-restore-window = Restore
action-minimize-window = Minimize
action-fullscreen = Full Screen
action-exit-fullscreen = Exit Full Screen
action-trash = Move to Trash
action-run = Run
action-switch-window = Switch to Window
action-close-window = Close Window

## Media and power commands
system-media-play-pause = Play / Pause
system-media-next = Next Track
system-media-previous = Previous Track
system-media-subtitle = Control the current media player
system-power-performance = Power Profile: Performance
system-power-balanced = Power Profile: Balanced
system-power-saver = Power Profile: Power Saver
system-power-subtitle = Switch the system power profile

## Process actions
action-end-process = End Process
action-force-kill = Force Kill

## Bluetooth devices and Wi-Fi networks
device-connect = Connect { $name }
device-disconnect = Disconnect { $name }
device-bluetooth = Bluetooth device
device-bluetooth-connected = Bluetooth device — connected
device-wifi = Wi-Fi network
device-wifi-connected = Wi-Fi network — connected

## Key hints shown in the action panel
key-enter = Enter

## Settings: plugin keywords and favorites
keyword-none = no keyword
section-favorites = Pinned results
favorites-none = Nothing pinned yet
favorites-hint = Press Ctrl+K on any result and choose Pin on Top.
favorites-remove = Remove
favorite-kind-application = Application
favorite-kind-file = File
favorite-kind-window = Window
favorite-kind-command = Command
favorite-kind-plugin = Plugin
favorite-kind-clipboard = Clipboard
favorite-kind-link = Link

## Result subtitles carrying counts. Plurals are selected by Fluent, never
## by Rust: `format!("{n} lines")` says "1 lines", and the rules differ per
## language in ways an `if count == 1` cannot express.
window-subtitle = Window — { $app }
clipboard-chars = Clipboard — { $chars ->
        [one] { $chars } character
       *[other] { $chars } characters
    }
clipboard-lines-chars = Clipboard — { $lines ->
        [one] { $lines } line
       *[other] { $lines } lines
    }, { $chars ->
        [one] { $chars } character
       *[other] { $chars } characters
    }

## Motion
reduce-motion = Reduce motion

## Settings: search providers
section-providers = Search providers
provider-windows = Open windows
provider-system = System commands
provider-devices = Bluetooth and Wi-Fi
provider-clipboard = Clipboard history (clip)
provider-emoji = Emoji (emoji)
provider-web = Quicklinks and web searches
provider-calculator = Calculator (=)

## Calculator
calc-copy-subtitle = Copy the answer

## Shown until the launcher has been used once.
first-run-hint = Type to search · Ctrl+K for actions · try “clip”, “emoji”, “kill”, or “= 2+2”

## The frecency inspector, shown under the action panel.
ranking-never-used = Score { $score } · never used, so no usage boost
ranking-used = Score { $score } · used { $count ->
        [one] once
       *[other] { $count } times
    }, last { $days ->
        [0] today
        [one] yesterday
       *[other] { $days } days ago
    } · recency weight { $recency }
