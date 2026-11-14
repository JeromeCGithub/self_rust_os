"""Complete terminal records from the QEMU serial byte stream."""
import re


def halt_record(serial: str) -> str | None:
    """Return one complete kernel halt record; incomplete output is still pending."""
    match = re.search(r'^\[system-halt\][^\r\n]*\r?\n', serial, re.MULTILINE)
    return match.group(0).rstrip('\r\n') if match else None
