#!/usr/bin/env python3
"""Prepare an exact, checksummed release review from built main/runtime artifacts.

This command performs no network request or publication. CI can upload the fully
validated review to a new draft only after its source/tag checks pass.
"""
import argparse
import json
from pathlib import Path
import re
import shutil
import tempfile
import zipfile
from distribution_notices import sha
from installer_support import payload


def prepare(directory, destination, tag, revision):
    if destination.exists() and (not destination.is_dir() or any(destination.iterdir())):
        raise ValueError('Release review output must be a new or empty directory')
    if destination.resolve().is_relative_to(directory.resolve()):
        raise ValueError('Release review output must be outside the asset input directory')
    if not re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9.-]+)?',tag) or not re.fullmatch(r'[a-f0-9]{40}',revision):
        raise ValueError('Invalid release tag or source revision')
    files={}
    for path in sorted(directory.rglob('*')):
        if not path.is_file():continue
        if path.is_symlink() or not re.fullmatch(r'[A-Za-z0-9_.+-]{1,180}',path.name) or path.name in files:
            raise ValueError('Unsafe or duplicate release asset name')
        files[path.name]=path
    source=files.get('build-source.json')
    if not source:raise ValueError('Build source provenance is missing')
    provenance=json.loads(source.read_text())
    if provenance['sourceRevision']!=revision:raise ValueError('Build source revision differs from requested release')
    if provenance['sourceDirty']:raise ValueError('Release assets were built from modified source; commit and build the intended tag first')
    primary={name:path for name,path in files.items() if name.endswith(('.tar.gz','.AppImage','.snap','.zip'))}
    if len(primary)!=4 or any(sum(name.endswith(suffix) for name in primary)!=1 for suffix in ('.tar.gz','.AppImage','.snap','.zip')):
        raise ValueError('A release needs one main archive, AppImage, Snap and separate Java runtime ZIP')
    allowed={'build-source.json'}
    for name,path in primary.items():
        sidecar=files.get(name+'.sha256')
        if not sidecar or sidecar.read_text()!=sha(path.read_bytes())+'  '+name+'\n':
            raise ValueError('Release asset checksum differs or sidecar is missing')
        allowed.update((name,name+'.sha256'))
    archive=next(path for name,path in primary.items() if name.endswith('.tar.gz'))
    with tempfile.TemporaryDirectory(prefix='kode-release-review-') as temporary:
        package,manifest=payload(archive,Path(temporary))
        manifest_sha=sha((package/'package-manifest.json').read_bytes())
        if tag!=manifest['version']:raise ValueError('Release tag does not match the compiled package version')
        if provenance['binarySha256']!=manifest['files']['kode']['sha256']:
            raise ValueError('Build provenance does not describe this executable')
    base_metadata=archive.name.removesuffix('.tar.gz')+'.manifest.json'
    if base_metadata not in files or sha(files[base_metadata].read_bytes())!=manifest_sha:
        raise ValueError('Main archive manifest sidecar differs from the actual payload')
    allowed.add(base_metadata)
    for suffix in ('.AppImage','.snap'):
        path=next(path for name,path in primary.items() if name.endswith(suffix))
        metadata=files.get(path.name+'.manifest.json')
        if not metadata:raise ValueError('Installer manifest is missing')
        entry=json.loads(metadata.read_text())
        if entry['payloadArchiveSha256']!=sha(archive.read_bytes()) or entry['payloadManifestSha256']!=manifest_sha or entry['sha256']!=sha(path.read_bytes()) or entry['target']!=manifest['target']:
            raise ValueError('Installer describes a different payload')
        allowed.add(metadata.name)
    runtime=next(path for name,path in primary.items() if name.endswith('.zip'))
    if runtime.name!='kode-jdbc-runtime-'+manifest['target']+'.zip':
        raise ValueError('Runtime asset name does not match the application downloader')
    runtime_metadata=files.get(runtime.name.removesuffix('.zip')+'.manifest.json')
    if not runtime_metadata:raise ValueError('Java runtime manifest is missing')
    with zipfile.ZipFile(runtime) as container:
        encoded=container.read('runtime-manifest.json')
        entry=json.loads(encoded)
    if encoded!=runtime_metadata.read_bytes() or entry['target']!=manifest['target'] or entry['javaMajor']<21:
        raise ValueError('Java runtime manifest or target differs')
    allowed.add(runtime_metadata.name)
    if set(files)!=allowed:raise ValueError('Unexpected files in release input')
    destination.mkdir(parents=True,exist_ok=True)
    checksums=[]
    for name in sorted(allowed):
        path=files[name]
        shutil.copyfile(path,destination/name)
        checksums.append({'name':name,'sha256':sha(path.read_bytes()),'size':path.stat().st_size})
    review={'schemaVersion':1,'tag':tag,'sourceRevision':revision,'target':manifest['target'],'minimumGlibc':manifest['minimumGlibc'],'javaRuntimeSeparate':True,'assets':checksums}
    (destination/'release-manifest.json').write_text(json.dumps(review,sort_keys=True,indent=2)+'\n')
    (destination/'SHA256SUMS').write_text(''.join(f"{entry['sha256']}  {entry['name']}\n" for entry in checksums))
    notes=f"Kode {manifest['version']} native Linux distribution\n\nSource: {revision}\nTarget: {manifest['target']}, glibc {manifest['minimumGlibc']}+.\nJava 21+ is external/optional; the Java runtime ZIP is separate from the main application.\n\nReview the package manifests, checksums and qualification results before publishing this draft.\n"
    (destination/'RELEASE-NOTES.md').write_text(notes)
    return review


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--tag',required=True)
    parser.add_argument('--source-revision',required=True)
    args=parser.parse_args()
    review=prepare(args.directory,args.output,args.tag,args.source_revision)
    print(json.dumps({'tag':review['tag'],'assets':len(review['assets']),'publicationPerformed':False}),flush=True)


if __name__=='__main__':main()
