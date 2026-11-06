"""Finalize only the length fields in QEMU's fixed PCM WAV capture format."""
import struct

def finalize(raw):
    if len(raw)<44 or (len(raw)-44)%4:
        raise ValueError('incomplete PCM capture')
    fields=struct.unpack('<4sI4s4sIHHIIHH4sI',raw[:44])
    if fields[:1]!=(b'RIFF',) or fields[2:12]!=(b'WAVE',b'fmt ',16,1,2,48000,192000,4,16,b'data'):
        raise ValueError('unsupported capture format')
    if fields[1] not in (0,len(raw)-8) or fields[12] not in (0,len(raw)-44):
        raise ValueError('capture length mismatch')
    result=bytearray(raw)
    struct.pack_into('<I',result,4,len(raw)-8)
    struct.pack_into('<I',result,40,len(raw)-44)
    return bytes(result)
