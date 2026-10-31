"""Private QEMU test images and bounded control-socket startup."""
from pathlib import Path
import os
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


def disk_image(root):
    """Return the integration fixture selected for this test run."""
    return Path(os.environ.get('RUSTOS_TEST_DISK', Path(root) / 'target/grub-disk.img'))


class DeferredWorkers:
    """Start request-timeout fixtures once their guest transport phase begins."""
    def __init__(self, workers, marker):
        self.workers = workers
        self.marker = marker
        self.started = False

    def observe(self, output):
        if not self.started and self.marker in output:
            self.started = True
            for worker in self.workers:
                worker.start()
