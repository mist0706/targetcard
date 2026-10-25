# Recording labelled traffic

`targetcard-record` exists to answer one question: **which packet carries the
player you selected?** The card currently guesses (tracked target, then last
damage, then newest spawn) because the selection opcode is unknown. Finding it
needs traffic labelled with the exact moment you click.

The tool records what the parser already sees, in the same text format
`share::read_capture` and `a2t-probe opcodes|find|diff` read, then uploads the
bundle to a collector on your own network. Nothing is sent anywhere else.

## Get it

```
https://github.com/mist0706/targetcard/releases/download/record/targetcard-record.exe
```

Built by `.github/workflows/record.yml` on windows-latest and republished on every
change to the tool or the parser. No Rust, no Python, no zip.

## Run it

In an **Administrator** PowerShell, with Aion 2 running and your character in a
place with other players:

```powershell
.\targetcard-record.exe --seconds 90 --upload http://10.7.1.235:8791 --token TOKEN
```

Then, while it records:

| do this | then |
|---|---|
| click player A | type their name, press Enter |
| click empty ground | press Enter on an empty line |
| click player B | type their name, press Enter |
| click player C | type their name, press Enter |
| open the character-info / inspect window on C, close it | press Enter on an empty line |

`q` and Enter stops early. It also stops on its own after `--seconds`.

## What it writes

| file | contents |
|---|---|
| `packets_<stamp>.txt` | `<rfc3339>|<stream>|<hex>` per TCP payload, the format `a2t-probe` reads |
| `markers.jsonl` | one object per marker: index, at_ms, label, and the name table at that moment |
| `manifest.json` | host, window, payload counts, whether capture was available |

The name table in each marker is the point: it turns "I clicked Sins" into an
entity id, so the packets right after the marker can be searched for it.

## Privacy

It captures through the meter's own pipeline, which only feeds the port the game
speaks on. Output goes to `--upload` or stays in `./recordings`. The collector
stores files and serves nothing back but file names and sizes.

## The app's own log

The app can push its debug log to the same collector:

```powershell
.\build-targetcard.ps1 -BuildOnly -Force -CollectorToken TOKEN
```

That writes `%APPDATA%\com.targetcard.desktop\diagnostics.json` once; later builds
reuse it, and the token stays out of this repository. With it present the app
turns debug logging on by itself and uploads the **last megabyte of
`debug.log`, gzipped, every five minutes, but only when the log has changed**.

Sessions land in `~/aion-captures/<HOST>-logs/` on the collector. Delete
`diagnostics.json` to stop, and nothing is sent again.

What is in there: lookup outcomes and reasons, capture device and port choices,
zone and party events, player names the parser saw, timings. No passwords and no
game credentials, because the app never has any.
