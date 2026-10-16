#!/usr/bin/env python3
"""CI-only new-draft publication after artifact/source/tag validation.

No automatic publication, existing release modification, tag creation or clobber.
Local review uses release-assets.py without making any network request.
"""
import argparse
import importlib.util
import json
from pathlib import Path
import re
import subprocess
from distribution_notices import ROOT


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--tag',required=True)
    parser.add_argument('--repository',required=True)
    args=parser.parse_args()
    if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+',args.repository):
        raise ValueError('Invalid release repository')
    revision=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip()
    spec=importlib.util.spec_from_file_location('release_assets',ROOT/'scripts/release-assets.py')
    assets=importlib.util.module_from_spec(spec);spec.loader.exec_module(assets)
    assets.prepare(args.directory,args.output,args.tag,revision)
    # Resolve the existing tag to a commit, including annotated tags. This API
    # call and verify-tag prevent implicit tag creation on a different branch.
    remote=json.loads(subprocess.check_output(['gh','api',f'repos/{args.repository}/commits/{args.tag}'],text=True))
    if remote['sha']!=revision:
        raise ValueError('Remote tag does not identify the built source commit')
    existing=subprocess.run(['gh','api',f'repos/{args.repository}/releases/tags/{args.tag}'],capture_output=True,text=True)
    if existing.returncode==0:
        raise ValueError('A release already exists for this tag; existing drafts and published assets are never overwritten')
    if 'HTTP 404' not in existing.stderr:
        raise ValueError('Cannot verify that the release tag has no existing release')
    files=sorted(str(path) for path in args.output.iterdir() if path.is_file())
    subprocess.run(['gh','release','create',args.tag,*files,'--repo',args.repository,'--draft','--verify-tag','--target',revision,'--title','Kode '+args.tag,'--notes-file',str(args.output/'RELEASE-NOTES.md')],check=True)
    print('Created draft with verified assets; publication remains an explicit release action',flush=True)


if __name__=='__main__':main()
