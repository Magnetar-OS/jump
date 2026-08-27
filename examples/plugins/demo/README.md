# demo plugin

Minimal jump plugin, used to verify the plugin host end to end.

```sh
cp -r examples/plugins/demo ~/.local/share/jump/plugins/demo
```

Then type `demo hello` in the launcher. `search.sh` echoes the query back as two
results; `activate` appends the selected `arg` to `/tmp/jump-demo-plugin.log`.
