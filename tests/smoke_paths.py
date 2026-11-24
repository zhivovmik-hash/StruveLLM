"""Locate the build under test; keep each controller in its own writable root."""
import os
from pathlib import Path
import shutil
import tempfile


def test_cli():
    target = Path(os.environ.get('CARGO_TARGET_DIR', Path(__file__).resolve().parents[1] / 'target'))
    choices = [os.environ.get('STRUVE_TEST_CLI'), target / 'release/struve-llm.exe',
               target / 'debug/struve-llm.exe', shutil.which('struve-llm.exe')]
    for choice in choices:
        if choice and Path(choice).is_file():
            return Path(choice).resolve()
    raise RuntimeError('Build struve-llm or set STRUVE_TEST_CLI')


def test_root(prefix, with_node=True):
    root = Path(tempfile.mkdtemp(prefix=prefix, dir=os.environ.get('STRUVE_TEST_TMP')))
    if with_node:
        node = os.environ.get('STRUVE_TEST_NODE') or shutil.which('node.exe')
        if not node:
            raise RuntimeError('Install Node or set STRUVE_TEST_NODE')
        dest = root / 'app/node.exe'
        dest.parent.mkdir()
        try:
            os.link(node, dest)
        except OSError:
            shutil.copy2(node, dest)
    return root


def compute_gpu(inventory, settings):
    reserved = settings.get('reserved_gpu_ids') or []
    available = [g for g in inventory['gpus'] if g.get('compute_backend') == 'cuda'
                 and g['id'] not in reserved and g.get('vram_gib', 0) - g.get('used_gib', 0) >= .5]
    if not available:
        raise RuntimeError('This hardware integration test requires an available CUDA GPU')
    return max(available, key=lambda g: g['vram_gib'] - g.get('used_gib', 0))
