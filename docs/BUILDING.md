# Building QuackOS with Claude Code

Two parts: **code** (Claude Code is great at this) and **the ISO** (Cubic is a GUI, you click through it).

## 1. Get set up

```bash
git clone https://github.com/clrealy/QuackOS
cd QuackOS
npm install -g @anthropic-ai/claude-code   # or follow the official install docs
claude
```

Claude Code reads `CLAUDE.md` automatically, so it already knows the rules, layout, and how to test.
Do this on a throwaway Ubuntu machine or a VM, **not** your daily driver.

## 2. Things to ask Claude Code

Start with whatever you want next. These work well, in order:

1. *"Run `./tools/test-main-fakeroot.sh`, `cargo test` in `duckfs/`, and build `carbs/`. Tell me what's red."*
2. *"Add move and resize to Carbs with pointer grabs, floating macOS-style. Test it headless like CLAUDE.md says and show me a screenshot."*
3. *"Add wlr-layer-shell support to Carbs so a panel can attach to the top of the screen."*
4. *"Create `bread/`, a Rust layer-shell client that draws a top bar with a clock and an app menu."*
5. *"Design the recovery environment ('The Pond') as a minimal initramfs on the RECOVERY partition that reinstalls from a manifest."*

Tip: tell it "don't say it works unless you ran it", the repo's `CLAUDE.md` already says this.

## 3. Build the ISO (Cubic)

1. Install Cubic: https://github.com/PJ-Singh-001/Cubic (it's in a PPA, see its README).
2. Download the **Ubuntu 26.04 LTS** desktop ISO.
3. Open Cubic, pick an empty project folder, choose the ISO, click through to the **virtual environment terminal** page.
4. Copy `build/main.sh` into the environment (Cubic has a copy button on that page), then in the terminal:
   ```bash
   chmod +x main.sh
   ./main.sh
   ```
   It needs internet inside the chroot. If the GURT step fails, the script prints the one-liner to finish after boot.
5. Continue through the Cubic pages and generate the ISO.
6. **Boot it in a VM first** (VirtualBox/QEMU/virt-manager), never straight onto your real disk.

## 4. The recovery partition

`main.sh` only *protects* a partition labeled `RECOVERY`. You create it at install time:
- make a small partition (a few GB), set its filesystem label **or** GPT partition name to `RECOVERY`
- it won't be mounted, will be hidden from file managers, and is read-only at the kernel level

The recovery *environment* that lives on it isn't built yet. That's roadmap item "The Pond".

## 5. Keep `peck` in sync

`build/main.sh` embeds a copy of `peck/peck`. After editing `peck/peck`, update the embedded copy
(or ask Claude Code to), then run `./tools/test-main-fakeroot.sh`, which fails if they differ.

## Honest status

`main.sh` has not been run in a real Cubic chroot yet. Expect to fix something on the first real build, that's normal 🦆
