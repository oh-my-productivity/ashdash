# Merges several changelog sections into one section body.
#
#   awk -v heading='## [0.2.0] - 2030-01-02' -f changelog-merge.awk generated.md draft1.md draft2.md ...
#
# Every input file is one section: an optional `## ...` heading line (ignored), optional introduction lines, and
# `### Name` subsections. The result is `heading`, the first non-empty introduction, and the subsections: the ones that
# git-cliff produces first, in their fixed order (Breaking changes, Added, Fixed, Performance, Changed, Documentation,
# Other), then any other subsection in order of first appearance. Entries of subsections with the same name are
# concatenated in file order. Blank lines inside entries are kept; leading and trailing blank lines are not.
# Deterministic: the same inputs give byte-identical output.

BEGIN {
  n_canon = split("Breaking changes|Added|Fixed|Performance|Changed|Documentation|Other", canon, "|")
  n_names = 0
  n_intro = 0
}

FNR == 1 { current = ""; in_intro = 1; file_intro = "" }

/^## / { next }

/^### / {
  name = substr($0, 5)
  sub(/[[:space:]]+$/, "", name)
  if (!(name in body)) {
    names[++n_names] = name
    body[name] = ""
  }
  current = name
  in_intro = 0
  next
}

{
  if (in_intro) {
    file_intro = file_intro $0 "\n"
    intro_files[FILENAME] = file_intro
    if (!(FILENAME in intro_order_seen)) { intro_order_seen[FILENAME] = 1; intro_order[++n_intro] = FILENAME }
  } else {
    body[current] = body[current] $0 "\n"
  }
}

function trim_blank(text) {
  sub(/^(\n)+/, "", text)
  while (text ~ /\n\n+$/) sub(/\n\n+$/, "\n", text)
  return text
}

function emit(name) {
  text = trim_blank(body[name])
  if (text == "") return
  printf "\n### %s\n\n%s", name, text
  done[name] = 1
}

END {
  print heading
  for (i = 1; i <= n_intro; i++) {
    text = trim_blank(intro_files[intro_order[i]])
    if (text != "") {
      printf "\n%s", text
      break
    }
  }
  for (i = 1; i <= n_canon; i++) if (canon[i] in body) emit(canon[i])
  for (i = 1; i <= n_names; i++) if (!(names[i] in done)) emit(names[i])
}
