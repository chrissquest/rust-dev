# ff-export

An experimental Nintendo DS asset extraction pipeline: point it at a DS ROM and it pulls the game's
assets out for use elsewhere. Currently aimed at Fossil Fighters.

## Status

Work in progress. The container, compression, mesh, skeleton, animation and texture layers work, and
`export-all` writes every creature in the game out in one pass - 116 creatures, 698 clips, 229 texture
sheets, about 113 MB of `.glb` - verifying each file by importing it again as it goes.

| Milestone | Scope | State |
| --- | --- | --- |
| M0 | ROM header, FNT, FAT | done |
| M1 | MAR archives, MCM chunk wrapper, LZ10, Huffman | done |
| M2 | `3CL` manifest → creature → mesh / animation / texture, plus creature names | done |
| M3 | mesh + skeleton → static `.glb` | done — verified against the reference numbers and re-read with the glTF crate's importer, for every creature |
| M4 | animation clips in the `.glb` | done — one glTF animation per clip, named |
| M5 | textures → PNG, embedded, materials named | done — every palette is checked entry for entry against the reference, and every material group across all 116 creatures resolves to its image |
| M6 | every creature exported + `manifest.json` | done — all 698 clips export, reload and are catalogued |

## Output layout

```text
out/
  glb/         breme_030_attack.glb, breme_030_roar.glb, ...   one file per clip
  textures/    breme_030/din030_a.png, din030_b.png            loose sheets, one directory per creature
  manifest.json                                                 the index: creature -> clips, sheets, extents
```

**A clip is the unit, not a creature.** Every clip in this game names its own mesh, and those meshes are
not copies: they carry different bind poses - up to 14 units apart between two clips of the same
creature - and for two creatures a different number of bones. A mesh therefore only makes sense with the
animation it was authored for, which is also why the reference tool wrote one file per clip
(`vivosaur N animation M.usda`). The textures, by contrast, are per creature: all of a creature's clips
share one set of sheets, so those are written once and embedded in each of its `.glb` files.

## Usage

```text
ff-export info     <rom>                       ROM header, file and archive counts
ff-export list     <rom> [substring]           list ROM files (id, size, path)
ff-export unpack   <rom> <path> <outdir>       decompress one MAR archive into NNNN.bin
ff-export verify   <rom> <path> <refdir>       diff that output against a reference tree
ff-export manifest <rom> [limit]               creature -> mesh/animation/texture mapping
ff-export names    <rom> [all]                 the 116 creature id -> name pairs
ff-export mesh     <rom> <creature> [clip]     parse one creature mesh and report its geometry
ff-export obj      <rom> <creature> [clip] <file.obj>  write the decoded mesh as Wavefront OBJ
ff-export export   <rom> <creature> <clip> <file.glb>  write one clip: its mesh, skeleton, animation and textures
ff-export export-all <rom> <outdir>            export every creature: glb/, textures/ and manifest.json
ff-export textures <rom> <creature> <outdir>   write a creature's texture images out as loose PNGs
ff-export anim     <rom> <creature> <clip>     inspect one clip's animation block
ff-export hexdump  <rom> <path> <entry> [len]  raw stored bytes of one entry, for analysis
ff-export help                                 show this text
```

Example:

```powershell
cargo run --release -- info "Fossil Fighters (USA).nds"
cargo run --release -- unpack "Fossil Fighters (USA).nds" model/battle/arcdin .\out\arcdin
cargo run --release -- export-all "Fossil Fighters (USA).nds" .\out\ff1_usa
cargo run --release -- export "Fossil Fighters (USA).nds" 30 1 .\out\breme_attack.glb
```

`export` needs a clip slot because each clip names its own mesh: `export <rom> 30 1 <file>` is Breme's
attack, and the error message lists the slots a creature actually has.

## Clip names

A creature's clips are named by slot. Slot 0 is always unused, and a creature carries between four
and seven of the remaining slots.

| Slot | Name | Notes |
| --- | --- | --- |
| 1 | `attack` | the whole combo; the game truncates it at run time for a one, two or three hit attack |
| 2 | `attack-secondary` | a distinct secondary attack; only 36 of the 116 creatures have one |
| 3 | `roar` | |
| 4 | `cheer` | |
| 5 | `hurt` | |
| 6 | `hurt-critical` | |
| 7 | `idle` | |

## Requirements

Rust 2024 edition, and a ROM you obtained legally. No game data is committed to this repository.

