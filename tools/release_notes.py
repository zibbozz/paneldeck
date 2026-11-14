import re,sys,subprocess
v=sys.argv[1]
def rd(p): return open(p,encoding="utf-8").read()
def wr(p,t): open(p,"w",encoding="utf-8",newline="\n").write(t)
s=rd("Changelog.md")
titles=dict(l.strip().split("|",1) for l in open("tools/titles.txt",encoding="utf-8") if "|" in l)
sys.path.insert(0,"tools"); import changelog  # Keep a Changelog: Abschnitt [<v>] lesen
sec=[x for x in changelog.sections(s) if x[0]==v]
if not sec: sys.exit(f"Changelog.md hat keinen Abschnitt [{v}]")
body=sec[0][2].strip()
title=titles.get(v,'Paneldeck '+v)
sha=subprocess.run(["git","rev-list","-n","1",f"v{v}"],capture_output=True,text=True).stdout.strip()
out=f"# Release v{v}\n\n**Tag:** `v{v}`\n\n**Ziel-Commit:** `{sha}`\n\n_So anlegen: GitHub → Releases → Draft a new release → „Choose a tag“ → `v{v}` eintippen → „Create new tag on publish“ → unter „Target“ den Tab „Recent commits“ öffnen und den Commit `{sha[:7]}` wählen._\n\n**Titel:** {title}\n\n**Beschreibung:**\n\n{body}\n\n---\nAnhang: `paneldeck-{v}-windows-x64.zip` (portabel: `paneldeck.exe` plus Lizenzen und README, Windows x64, offline lauffähig) und `Paneldeck-Setup-{v}.exe` (Installer, pro Benutzer, ohne Admin, mit Lizenzen). Beim ersten Start ggf. SmartScreen: „Weitere Informationen“ → „Trotzdem ausführen“.\n"
wr(f"dist/release-notes-{v}.md",out)
# Nur Titel und Text, direkt für `gh release create --title/--notes-file`
wr(f"dist/gh-title-{v}.txt",title)
wr(f"dist/gh-notes-{v}.md",body+"\n")
