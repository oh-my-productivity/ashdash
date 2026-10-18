#!/usr/bin/env python3
"""Seed a throw-away Ashdash data folder for the lab QA run. usage: fixtures.py <HOME> <work dir>
Everything is synthetic. Creates workflows (folders under <HOME>/Library/Application Support/ashdash/workflows):
  qa.rerun   keyword `rr`  Script Filter with rerun 1: rows A,B,C for its first two runs, then C,A,B (stable uids; counter file <work>/rr-count); Return writes the row's arg to <work>/picked.txt
  qa.slow    keyword `sl`  Script Filter that sleeps 3 s (runningsubtext "Slow..."), three rows; Return writes the arg to <work>/picked.txt
  qa.garbage keyword `gb`  prints garbage; qa.hang keyword `hg` sleeps 30 s
  qa.pick    keyword `pk`  twelve static rows Pick 01..12 (arg P01..P12); Return writes the arg to <work>/picked.txt
  qa.long    keyword `lt`  one row whose Large Type text is 3,000 numbered words (to see the Large Type scroll)
  qa.mods    keyword `md`  one row with cmd/alt/shift variants (subtitle + arg)
  qa.copy    keywords `cp` (transient Copy to Clipboard), `cq` (plain copy), `nt` (notification)
  qa.trigger external trigger `go` -> Write File <work>/trigger.txt (arg); qa.url external trigger `u` -> Open URL (consent dialog)
plus three snippets and one file tree under <work>/files.
"""
import json, os, plistlib, sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "scripts"))
import format_ids as ids  # noqa: E402  (identifiers of the workflow package format)

home, work = sys.argv[1], sys.argv[2]
data = os.path.join(home, "Library/Application Support/ashdash")
wfs = os.path.join(data, "workflows")
os.makedirs(wfs, exist_ok=True)
os.makedirs(work, exist_ok=True)
picked = os.path.join(work, "picked.txt")


def wf(folder, name, bundle, objects, connections):
    d = os.path.join(wfs, folder)
    os.makedirs(d, exist_ok=True)
    with open(os.path.join(d, "info.plist"), "wb") as f:
        f.write(plistlib.dumps({"name": name, "bundleid": bundle, "createdby": "QA", "description": "synthetic QA fixture",
                                "objects": objects, "connections": connections}))


def sf(uid, kw, script, running=""):
    return {"uid": uid, "type": ids.INPUT_SCRIPTFILTER,
            "config": {"keyword": kw, "withspace": True, "argumenttype": 1, "type": 0, "scriptargtype": 1,
                       "title": kw, "subtext": "", "runningsubtext": running, "script": script}}


def run(uid, script):
    return {"uid": uid, "type": ids.ACTION_SCRIPT, "config": {"type": 0, "script": script, "concurrently": False, "scriptargtype": 1}}


pick = f'printf "%s" "$1" > "{picked}"'
cnt = os.path.join(work, "rr-count")
rr = (f'n=$(cat "{cnt}" 2>/dev/null || echo 0); n=$((n+1)); echo $n > "{cnt}"; '
      'if [ $n -lt 3 ]; then o="A B C"; else o="C A B"; fi; out=""; i=0; for k in $o; do '
      'out="$out{\\"uid\\":\\"row$k\\",\\"title\\":\\"Row $k\\",\\"subtitle\\":\\"pos $i run $n\\",\\"arg\\":\\"$k\\"},"; i=$((i+1)); done; '
      'echo "{\\"rerun\\":1,\\"items\\":[${out%,}]}"')
wf("qa.rerun", "QA rerun", "qa.rerun", [sf("S", "rr", rr), run("R", pick)], {"S": [{"destinationuid": "R", "modifiers": 0}]})
slow = ('sleep 3; echo \'{"items":[{"uid":"s1","title":"Slow one","arg":"s1"},{"uid":"s2","title":"Slow two","arg":"s2"},'
        '{"uid":"s3","title":"Slow three","arg":"s3"}]}\'')
wf("qa.slow", "QA slow", "qa.slow", [sf("S", "sl", slow, "Slow…"), run("R", pick)], {"S": [{"destinationuid": "R", "modifiers": 0}]})
wf("qa.garbage", "QA garbage", "qa.garbage", [sf("S", "gb", "echo 'this is {not json'; echo 12345")], {})
wf("qa.hang", "QA hang", "qa.hang", [sf("S", "hg", "sleep 30")], {})
mods = ('echo \'{"items":[{"uid":"m1","title":"Mods row","subtitle":"plain subtitle","arg":"plain","mods":{'
        '"cmd":{"subtitle":"CMD subtitle","arg":"cmd"},"alt":{"subtitle":"ALT subtitle","arg":"alt"},"shift":{"subtitle":"SHIFT subtitle","arg":"shift"}}}]}\'')
wf("qa.mods", "QA mods", "qa.mods", [sf("S", "md", mods), run("R", pick)],
   {"S": [{"destinationuid": "R", "modifiers": m} for m in (0, 524288, 1048576, 131072)]})
rows = ",".join('{"uid":"p%02d","title":"Pick %02d","subtitle":"row %d","arg":"P%02d"}' % (i, i, i, i) for i in range(1, 13))
wf("qa.pick", "QA pick", "qa.pick", [sf("S", "pk", "echo '{\"items\":[%s]}'" % rows), run("R", pick)],
   {"S": [{"destinationuid": "R", "modifiers": 0}]})
wf("qa.trigger", "QA trigger", "qa.trigger",
   [{"uid": "T", "type": ids.TRIGGER_EXTERNAL, "config": {"triggerid": "go"}},
    run("R", f'printf "%s" "$1" > "{work}/trigger.txt"')], {"T": [{"destinationuid": "R", "modifiers": 0}]})
wf("qa.url", "QA url", "qa.url",
   [{"uid": "T", "type": ids.TRIGGER_EXTERNAL, "config": {"triggerid": "u"}},
    {"uid": "U", "type": ids.ACTION_OPENURL, "config": {"url": "https://example.invalid/{query}"}}],
   {"T": [{"destinationuid": "U", "modifiers": 0}]})

lt = 'text=$(for i in $(seq 1 3000); do printf "L%03d " $i; done); echo "{\\"items\\":[{\\"title\\":\\"Long text\\",\\"valid\\":false,\\"text\\":{\\"largetype\\":\\"$text\\"}}]}"'
wf("qa.long", "QA long", "qa.long", [sf("S", "lt", lt)], {})


def kw(uid, word, title):
    return {"uid": uid, "type": ids.INPUT_KEYWORD,
            "config": {"keyword": word, "withspace": True, "argumenttype": 1, "text": title, "subtext": "QA keyword"}}


wf("qa.copy", "QA copy", "qa.copy",
   [kw("K1", "cp", "Copy transient"), {"uid": "C1", "type": ids.OUTPUT_CLIPBOARD, "config": {"clipboardtext": "TRANSIENT-{query}", "transient": True}},
    kw("K2", "cq", "Copy plain"), {"uid": "C2", "type": ids.OUTPUT_CLIPBOARD, "config": {"clipboardtext": "PLAINCOPY-{query}", "transient": False}},
    kw("K3", "nt", "Notify"), {"uid": "N1", "type": ids.OUTPUT_NOTIFICATION, "config": {"title": "QA", "text": "hello {query}"}}],
   {"K1": [{"destinationuid": "C1", "modifiers": 0}], "K2": [{"destinationuid": "C2", "modifiers": 0}], "K3": [{"destinationuid": "N1", "modifiers": 0}]})

# files for path browsing, file actions and the trash test
files = os.path.join(work, "files")
os.makedirs(os.path.join(files, "sub"), exist_ok=True)
for n, body in (("report.txt", "hello report\n"), ("notes.txt", "notes\n"), ("trashme.txt", "bye\n")):
    open(os.path.join(files, n), "w").write(body)

# snippets (docs/SETTINGS.md: a flat list of {"name","keyword","text"})
snip = [{"name": "qa sig", "keyword": "qasig", "text": "Regards from QA {date}"},
        {"name": "qa plain", "keyword": "qaplain", "text": "PLAIN-SNIPPET-TEXT"},
        {"name": "qa clip", "keyword": "qaclip", "text": "got:{clipboard}"}]
json.dump(snip, open(os.path.join(data, "snippets.json"), "w"))
print("seeded", data)
