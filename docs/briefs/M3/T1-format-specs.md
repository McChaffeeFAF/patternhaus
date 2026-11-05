# Brief M3/T1: Sampler format specifications (clean room, spec role)

**Repo:** `E:\patternhaus`. Work in the worktree `local/wt/m3-t1` on branch `m3/t1-format-specs`. Set `CARGO_TARGET_DIR` to `E:\patternhaus\target`.

**Read first:**
- `AGENTS.md`: the clean-room policy and the local layout;
- `docs/DESIGN.md` §8;
- `docs/milestones/M3.md`, especially "Clean-room rule for formats";
- `docs/formats/flp.md`, the M2 spec. Match its style and its evidence marks.

## Your role in the clean room

You are the **spec writer**. Other agents will implement the importers from your documents alone. Each spec must be complete and precise, and must state **facts only**:
- byte layouts and chunk ids;
- opcode names, value ranges, units and defaults;
- how each feature appears on disk.

The specs contain no code and no copied prose.

**Allowed sources:**
- Public specifications:
  - the SFZ format documentation (sfzformat.com: opcodes, headers, versions);
  - the SoundFont 2.04 technical specification;
  - the DecentSampler file-format documentation.
- Public articles, wikis and forum posts.
- Open-source converters, such as ConvertWithMoss (LGPL-3.0), **for facts only**. Don't copy or translate their code, and don't reproduce their structure. Cite what you learned and where.
- Analysis of files you generate yourself or that are freely redistributable.
- **The owner's local libraries**, read-only, if the owner gives a folder. The orchestrator will pass it on. Until then, mark facts by evidence level.

**Forbidden:**
- decompiling or disassembling Kontakt, Logic or any other binary;
- anything about breaking or bypassing encryption. For encrypted files, specify only how to **detect** that a file is encrypted or licence-locked, so the importer can refuse it.
- copying wording from any source;
- committing third-party files.

**Evidence marks:** label every fact with one of: "per spec", "confirmed on files", "documented, not seen" or "inferred".

## Deliverables (in this order, committing as each is done)

1. **`docs/formats/sfz.md`.** Headers and inheritance (`<control> <global> <master> <group> <region>`, plus `<curve>` and `<effect>`); `#define`, `#include` and path rules; and the opcode table for v1 plus the common ARIA/v2 opcodes. For each opcode give its type, range, unit, default and engine meaning. Include note-name parsing, the trigger modes (attack, release, first, legato), `seq_length`/`seq_position`, `lorand`/`hirand`, keyswitch opcodes, the `sw_*` family, CC conditions and crossfade opcodes (`xfin_*`, `xfout_*`, `xf_cccurve`), loop opcodes, envelope generators (`ampeg_*`, `fileg_*`), LFOs, and filters. Mark which opcodes M3 must support and which it may list as skipped.
2. **`docs/formats/sf2.md`.** The RIFF layout (INFO, sdta and pdta: phdr, pbag, pmod, pgen, inst, ibag, imod, igen, shdr); generator ids, units and the preset/instrument summing rules; default modulators; sample links (stereo pairs, ROM samples); and SF3 (Ogg-compressed samples, and how shdr changes).
3. **`docs/formats/dspreset.md`.** The XML schema: groups, samples, attributes, effects, UI elements kept as data, tags, and the round-robin and trigger attributes.
4. **`docs/formats/exs.md`.** The EXS24 binary layout (header, zones, groups, samples, params), as far as public sources establish it. Mark the evidence level, since files may not be available.
5. **`docs/formats/nki.md`.** The Kontakt container family:
   - `.nki`, `.nkm`, `.nkb`, `.nkx`/`.nkc` monoliths and `.ncw`;
   - the version families (Kontakt 1.5/2, 4.2+, 5–7);
   - the instrument data needed for zones (key and velocity ranges, root, tune, volume, pan, sample start/end), loops, group membership and sample references;
   - NCW compression, which is a lossless sample codec;
   - how monoliths index their samples;
   - **how to detect encrypted or licence-locked content**, so it can be refused. Nothing about getting past it.

   Also list the version differences, and add an unknowns section with the evidence collected so far.

Also:
- **`docs/parity/haus-sampler.md`:** a per-format feature table with the columns supported, approximated, skipped and test, to be filled in by the implementers. Seed the rows from your specs.
- **A fixture plan:** for each format, which generated fixture files the implementers should build, and which features each one exercises. The fixtures are built by test code or `tools/soundgen`, never copied from anywhere.

## Rules
- Write docs only, plus at most a small probe tool under `tools/`, following `tools/flp-probe`, if it helps you analyse files. A probe's tests use tiny synthetic byte sequences built in the test.
- **Build load:** at most 4 compile jobs, one build at a time.
- **Commits:** commit increments with `git commit -s` and a `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>` line. Don't merge or push.

## Report back
- each spec: its coverage, its evidence levels and its main unknowns;
- which facts depend on files we don't have yet;
- the commit list.
