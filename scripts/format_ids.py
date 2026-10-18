"""The identifiers of the workflow package and appearance file formats, for the QA scripts.

The single source is crates/core/src/format_ids.rs; this module reads the constants from
it, so no script has to spell an identifier out. Usage:

    sys.path.insert(0, "<repo>/scripts"); import format_ids as ids
    ids.INPUT_KEYWORD     # object type of a keyword input
    ids.WORKFLOW_EXT      # file extension of a workflow package (no dot)
    ids.workflow_file("sample")   # "sample.<ext>"
"""
import pathlib
import re

_SOURCE = pathlib.Path(__file__).resolve().parent.parent / "crates" / "core" / "src" / "format_ids.rs"
_CONST = re.compile(r'^\s*pub const (\w+): &str = "([^"]*)";', re.M)

for _name, _value in _CONST.findall(_SOURCE.read_text()):
    globals()[_name] = _value


def workflow_file(stem):
    return f"{stem}.{globals()['WORKFLOW_EXT']}"


def appearance_file(stem):
    return f"{stem}.{globals()['APPEARANCE_EXT']}"


def snippets_file(stem):
    return f"{stem}.{globals()['SNIPPETS_EXT']}"
