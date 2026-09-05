================================================================
 Oblivion Remastered Save Decoder
 by Bram Haegeman
================================================================

WHAT THIS IS
------------
A small, free tool that unpacks an Oblivion Remastered save file
(.sav) into plain, readable text. Oblivion Remastered compresses
the actual game-state data inside your save using Unreal Engine's
Oodle compressor, so if you open a .sav file in a normal text or
hex editor, almost everything looks like random noise. This tool
decompresses that data and reads its real structure, so you (or
any modding tool) can actually see what's in it.

This is a read-only reverse-engineering tool. It does not edit,
patch, or write anything back into your save file, and it never
touches your live game saves — it only opens a copy of the bytes
to read them, and it never writes anything into your SaveGames
folder.


HOW TO USE IT
-------------
1. Run the .exe.
2. Click "Choose a .sav file..." and pick a save from:
   Documents\My Games\Oblivion Remastered\Saved\SaveGames
3. Wait a few seconds — longer for saves from long playthroughs,
   since there's simply more world state to unpack.
4. Two files appear next to this tool's .exe (never inside your
   SaveGames folder):
     <savename>_readable.txt  - a plain-English property listing
     <savename>_hexdump.txt   - the full raw byte-level dump

That's it. No install, no settings, nothing to configure.


WHAT'S IN THE OUTPUT FILES
----------------------------
_readable.txt is the one to start with. It lists every top-level
entry found in your save's data as "Name: Type = Value" — simple
values (numbers, on/off flags, text) are shown directly; complex
ones (structs, arrays, maps, which is where quest, item, NPC and
world data lives) are shown by name, type and size, since fully
decoding their internal layout is a bigger, separate effort (see
LIMITATIONS below). Even without that last step, this already
surfaces real, recognizable property names.

_hexdump.txt is the same decoded data in a classic offset/hex/
ASCII layout, for anyone who wants to inspect the raw bytes
directly — the format modding/reverse-engineering tools expect.

You'll see plenty of recognizable, real content in there —
including the game's actual internal save marker
("TES4SAVEGAME"), your full loaded plugin list (Oblivion.esm, the
DLCs, Knights.esp, etc.), and thousands of genuine Unreal Engine
"tagged properties" (StructProperty, ArrayProperty, and so on).
That's genuinely interesting on its own: it means Oblivion
Remastered's save data isn't some closed, proprietary blob — it's
serialized using Unreal's own well-documented property system,
just Oodle-compressed and never previously decoded.

Concretely, every save's data boils down to three top-level
entries:
  - OblivionData            (a few MB, ByteProperty array)
  - SaveGameDetails         (a VSaveGameDetails struct — this is
                              almost certainly where character
                              name/level/location/playtime and the
                              thumbnail live)
  - SerializedAltarSaveDataArray  (an ARRAY OF STRUCTS, several MB
                              — this has the shape of "one entry
                              per saved quest/NPC/world object,"
                              which is exactly where you'd expect
                              quest and inventory state to live)
That last one in particular is the natural next target for anyone
who wants to build a proper quest/inventory reader on top of this.

One small part of the save (containing your character's save-slot
thumbnail screenshot and some save-browser metadata: name, level,
location, playtime) is stored uncompressed rather than in the
Oodle chunk stream, so it isn't included in either output file —
it's already plain-readable if you look at the raw .sav file
directly with any hex editor.


HOW IT WORKS (SHORT VERSION)
-----------------------------
- The outer save file is a standard Unreal Engine "GVAS" save
  container. There's exactly one property inside it, a big byte
  array, holding the entire real save.
- That byte array is a sequence of individually Oodle-compressed
  128KB blocks, each with its own small header. This tool walks
  that chain block by block and decompresses each one, using an
  open-source, clean-room Oodle decoder (no proprietary SDK
  involved), stitching the results into one continuous buffer.
  However many blocks your save has — a fresh character or a
  400-hour one — it just keeps going until it runs out of data.
- What comes out is itself a nested Unreal save structure: a
  normal GVAS-style header, followed by a long list of "tagged
  properties" in Unreal's standard binary format (name, type,
  declared byte length, then the value). This tool walks that
  list top-to-bottom, decoding what it can and using each
  property's own declared length to skip cleanly past anything
  it doesn't fully understand yet — which is what makes it
  resilient to Bethesda's custom, undocumented struct types
  instead of just breaking on the first one.


LIMITATIONS (BEING HONEST)
----------------------------
This tool gets you all the way from "compressed noise" to a
readable list of every top-level property in your save, with
simple values fully decoded. It does NOT yet decode the *internal*
layout of the complex ones — the structs and arrays that hold
quest stages, inventory items, and NPC data use custom Bethesda/
Virtuos-defined schemas that aren't publicly documented, so this
tool reports them by name/type/size rather than guessing their
contents. That's a separate, genuinely bigger effort (mapping each
custom struct's fields one by one). Think of this as the
previously-missing first step — the actual save data, finally
readable and structurally mapped out — ready for someone (maybe
you!) to build the next layer on top of.


CREDITS & LICENSE
------------------
Built by Bram Haegeman. Licensed under PolyForm Noncommercial
1.0.0 — see LICENSE.txt. Free to use, modify and share for any
noncommercial purpose (personal use, research, modding, hobby
projects). Want to use it commercially? Get in touch with Bram
Haegeman first to arrange that.

Uses the open-source "oozextract" crate (MIT license) for Oodle
decompression, "gvas" (MIT license) for reading the Unreal Engine
save container, and "byteorder" (MIT/Apache) for the property-list
reader — no proprietary Oodle SDK or Bethesda/Virtuos code is
included or required. Those dependencies remain under their own
MIT/Apache terms regardless of this project's license.

If you build something further on top of this (a proper quest/
inventory parser, for instance), credit is appreciated.
