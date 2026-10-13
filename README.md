# dankmon

A small terminal system monitor: CPU, memory and swap, network, NFS client
and disks. No process list.

Colours come from your terminal's theme, which dankmon asks for at startup
(OSC 4, 10 and 11):

- Data colours are the theme's ANSI slots: red, green, yellow, blue, magenta
  and cyan, plus an orange when a bright slot holds a hue the six don't cover.
  Slots that are grey or would vanish against the background are skipped.
- Neutrals (borders, dim text, bar tracks, the pale cache band) are blended
  from the theme's background and foreground, on truecolor terminals.
- CPU cores cycle through the usable data colours.

Anything the theme can't supply falls back to built-in 256-colour choices.

```
cargo build --release
./target/release/dankmon            # or: -i 2 -w 300 --light / --dark
```

Keys: `q` or `Esc` quits, `t` cycles between the terminal's colours and the
built-in light and dark sets (`--light` / `--dark` start on those).

## Panels

- **CPU**: one line per logical core over time, with a matching coloured bar per
  core; total busy %, iowait and load average in the title.
- **Memory**: a stacked area over time. Used memory fills from the bottom,
  green, then amber above 75% and red above 90%. Reclaimable cache sits on
  top in a pale shade, and free memory is the empty space above. Available,
  buffers, dirty, and a swap bar when there is swap.
- **Network**: receive/transmit rate summed over physical NICs (bridges, veths
  and tunnels are left out of the sum, since that traffic also crosses a NIC).
  Per-interface rates and totals since boot; virtual interfaces appear only
  while active. Session totals in the title.
- **NFS client**: from `/proc/self/mountstats`, per mount: bytes moved over the
  wire vs bytes applications read and wrote (and the page-cache hit rate that
  implies), ops/s, average RTT and execution time, READ and WRITE latency,
  retransmissions, timeouts and errors, the busiest operations, and totals
  since mount. NFS traffic is also counted in the network panel.
- **Disks**: the same filesystems and columns as `df -hT` plus inode use, and
  total usage for local disks and network mounts (each device or export counted
  once).

`statvfs()` blocks on a hard NFS mount whose server is gone, so filesystem
usage is read on a background thread. If it stops reporting, the Disks panel
says so and the rest of the UI keeps running. The NFS panel reads only kernel
counters and never blocks.
