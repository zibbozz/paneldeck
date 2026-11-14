import re,sys
s=open("Changelog.md",encoding="utf-8").read()
titles=dict(l.strip().split("|",1) for l in open("tools/titles.txt",encoding="utf-8") if "|" in l)
sys.path.insert(0,"tools"); import changelog
out="# Release-Texte für GitHub\n\nAnhang je Release: die ZIP mit der passenden paneldeck.exe (Windows x64).\n\n"
for v,date,body in changelog.sections(s):
    if v=="Unreleased": continue
    body=body.strip()
    out+=f"---\n\n## v{v}\n\n**Tag:** `v{v}`\n\n**Titel:** {titles.get(v,'Paneldeck '+v)}\n\n**Text:**\n\n{body}\n\n"
open("dist/release-texte.md","w",encoding="utf-8",newline="\n").write(out)
