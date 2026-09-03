# Plugin directory

The store, without a store service. `jump` deliberately has no plugin registry
to run, no accounts and no update channel — a plugin is an executable and a
manifest in a directory. What a registry actually provides that a directory
does not is *discovery* and *some evidence the thing is not hostile*, and a
reviewed list in the repository provides both at a fraction of the moving
parts.

## Installing anything on this page

```sh
git clone <url> ~/.local/share/jump/plugins/<name>
jump plugin lint <name>          # checks the manifest and one sample query
```

Then type its keyword. Nothing needs restarting — plugins are discovered per
query.

Distribution packages install into `/usr/share/jump/plugins/<name>` instead,
which `jump` also searches. A plugin of the same name in the user's directory
shadows the packaged one, so copying a packaged plugin into `~/.local/share`
to modify it works the way you would expect.

## Read this before installing a plugin

A plugin is an arbitrary program that runs **as you**, on your keystrokes,
with your files and your session bus. The trust model is exactly `curl | sh`:
nothing sandboxes it, and `jump` cannot make it safe.

What `jump` does enforce is blast radius, not intent:

- a plugin that overruns its deadline (180 ms, or its own `timeout_ms` up to
  3 s) is killed and its results dropped;
- output over 1 MiB is discarded;
- a plugin that crashes, hangs or prints garbage degrades its own results and
  nothing else;
- any plugin can be switched off in Settings without deleting it.

None of that stops a plugin doing what it likes with your home directory.
Read the source of anything you install, prefer plugins whose source you can
read in a sitting, and treat a keyworded network plugin as something that
sends your keystrokes to a server.

## Bundled example

| Plugin | Keyword | What it does |
|---|---|---|
| [`examples/plugins/demo`](../examples/plugins/demo/) | `demo` | The reference implementation — copy it as a starting point |

## Community plugins

*Empty on purpose.* An entry is added here when it has been read, run and
linted, not when it is announced. Open a pull request adding a row with:

- the repository URL, and a licence,
- the keyword it claims,
- one line on what it does and what it talks to over the network,
- `jump plugin lint` output showing it clean.

A plugin that shells out to a package manager, writes outside its own
directory, or phones home without saying so in its description is not listed.

## Writing one

`jump plugin new <name>` scaffolds a working plugin; `jump plugin lint` checks
it. The format is a subset of Alfred's Script Filter schema — `uid`, `arg`,
`autocomplete`, `variables`, `mods` and `rerun` all behave as they do there —
so many existing Alfred workflow scripts port by writing a manifest. See the
[Plugins section of the README](../README.md#plugins) for the full contract.
