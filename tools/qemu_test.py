"""Private QEMU test images and bounded control-socket startup."""
from pathlib import Path
import shutil
import socket
import time


def connect(path, deadline, process):
    """Return a connected socket within the caller's existing startup deadline."""
    while True:
        if time.monotonic() >= deadline:
            raise TimeoutError(f'QEMU control socket not ready: {path}')
        if process.poll() is not None:
            raise RuntimeError('QEMU exited before its control socket was ready')
        connection = socket.socket(socket.AF_UNIX)
        connection.settimeout(3)
        try:
            connection.connect(str(path))
            return connection
        except (FileNotFoundError, ConnectionRefusedError):
            connection.close()
            time.sleep(.05)
        except BaseException:
            connection.close()
            raise


def copy_disk(source, directory):
    """Return a writable test image separate from the development template."""
    destination = Path(directory) / "disk.img"
    shutil.copyfile(source, destination)
    return destination
