"""Removes the DWARF debug sections (the last ones in the file) from a MinGW DLL.

Does what `strip --strip-debug` would do: the `.debug_*` sections (long names, stored as
"/NN" in the COFF string table) aren't loaded into memory; removing them only shortens the file.
Usage: strip_debug.py input.dll output.dll
"""
import sys

import pefile

src, dst = sys.argv[1], sys.argv[2]
pe = pefile.PE(src)
IMAGE_SCN_MEM_DISCARDABLE = 0x02000000

sections = list(pe.sections)
for s in sections:
    name = s.Name.rstrip(b"\0").decode(errors="replace")
    print(f"{name:10s} raw={s.SizeOfRawData / 1e6:8.2f} MB  vsize={s.Misc_VirtualSize / 1e6:8.2f} MB  flags={s.Characteristics:#x}")

# MinGW debug sections are named "/<offset>" and sit at the end of the file.
keep = len(sections)
while keep > 0 and sections[keep - 1].Name.startswith(b"/"):
    keep -= 1
removed = sections[keep:]
if not removed:
    sys.exit("no debug section at the end of the file")
assert all(s.Characteristics & IMAGE_SCN_MEM_DISCARDABLE for s in removed), "section is not discardable"

last = sections[keep - 1]
end_of_data = last.PointerToRawData + last.SizeOfRawData
align = pe.OPTIONAL_HEADER.SectionAlignment
size_of_image = (last.VirtualAddress + last.Misc_VirtualSize + align - 1) // align * align

pe.FILE_HEADER.NumberOfSections = keep
pe.FILE_HEADER.PointerToSymbolTable = 0
pe.FILE_HEADER.NumberOfSymbols = 0
pe.OPTIONAL_HEADER.SizeOfImage = size_of_image
# Zero the headers of the removed sections.
for s in removed:
    pe.set_bytes_at_offset(s.get_file_offset(), b"\0" * 40)
data = pe.write()[:end_of_data]
open(dst, "wb").write(data)
print(f"removed {len(removed)} sections; {len(data) / 1e6:.1f} MB")
