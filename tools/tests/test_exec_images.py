"""Build output must store initialized ELF bytes, with BSS described as RAM."""
from pathlib import Path
import re
import struct
import unittest
import importlib.util

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('executable_metadata', ROOT / 'tools/executable_metadata.py')
metadata_tool = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(metadata_tool)


class ExecutableImages(unittest.TestCase):
    def segments(self, program):
        binary = (ROOT / f'user_programs/{program}/{program}.bin').read_bytes()
        self.assertEqual(binary[:7], b'\x7fELF\x02\x01\x01', program)
        offset = struct.unpack_from('<Q', binary, 32)[0]
        size, count = struct.unpack_from('<HH', binary, 54)
        self.assertEqual(size, 56)
        loads = []
        for index in range(count):
            kind, flags, at, address, _, file_size, memory_size, align = struct.unpack_from(
                '<IIQQQQQQ', binary, offset + index * size)
            if kind == 1:
                self.assertLessEqual(file_size, memory_size)
                self.assertLessEqual(at + file_size, len(binary))
                loads.append((address, flags, file_size, memory_size))
        self.assertTrue(loads, program)
        return binary, loads

    def test_all_generated_programs_are_compact_elf_with_matching_metadata(self):
        for path in sorted((ROOT / 'user_programs').glob('*/*.bin')):
            with self.subTest(program=path.stem):
                _, loads = self.segments(path.stem)
                writable = min(address for address, flags, _, _ in loads if flags & 2)
                metadata = path.with_name(f'{path.stem}_meta.rs').read_text()
                readonly = int(re.search(r'READONLY_SIZE: usize = (\d+)', metadata)[1])
                self.assertEqual(readonly, (writable & ~4095) - 0x400000)

    def test_forge_heap_is_not_materialized_on_disk(self):
        binary, loads = self.segments('forge')
        zero_fill = sum(memory - stored for _, _, stored, memory in loads)
        self.assertGreaterEqual(zero_fill, 16 * 1024 * 1024)
        self.assertLess(len(binary), 1024 * 1024)

    def test_metadata_rejects_malformed_tables_and_segment_bounds(self):
        original, _ = self.segments('forge')
        for length in [0, 7, 63]:
            with self.assertRaises(ValueError):
                metadata_tool.readonly_size(original[:length])
        for offset, fmt, value in [(32, '<Q', 0), (32, '<Q', 2**64-1), (54, '<H', 55),
                                    (56, '<H', 0), (56, '<H', 129)]:
            with self.subTest(offset=offset, value=value):
                changed = bytearray(original)
                struct.pack_into(fmt, changed, offset, value)
                with self.assertRaises(ValueError):
                    metadata_tool.readonly_size(changed)

    def test_metadata_rounds_shared_got_page_down(self):
        binary, loads = self.segments('spin')
        self.assertEqual(metadata_tool.readonly_size(binary), 4096)
        self.assertTrue(any(address % 4096 for address, flags, _, _ in loads if flags & 2))
