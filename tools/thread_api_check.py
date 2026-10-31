#!/usr/bin/env python3
"""Compile real SDK consumers; accept Send closures and reject unsafe captures."""
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT=Path(__file__).resolve().parents[1]
CASES=[
    ('owned_send', 'let value=alloc::boxed::Box::new(7usize); let t=userlib::thread::spawn(move || *value).unwrap(); let _=t.join();', None),
    ('non_send', 'let value=alloc::rc::Rc::new(7usize); let _=userlib::thread::spawn(move || value);', 'E0277'),
    ('borrowed_stack', 'let value=7usize; let _=userlib::thread::spawn(|| value);', 'E0373'),
    ('guard_owner', 'let mutex=userlib::sync::Mutex::new(7usize); let guard=mutex.lock(); let _=userlib::thread::spawn(move || drop(guard));', 'E0277'),
]

def main():
    with tempfile.TemporaryDirectory(prefix='rustos-thread-api-') as directory:
        directory=Path(directory)
        (directory/'src').mkdir()
        manifest='[package]\nname="thread_api_probe"\nversion="0.1.0"\nedition="2021"\n[dependencies]\nuserlib={path='+json.dumps(str(ROOT/'user_programs/userlib'))+',features=["threads"]}\n'
        (directory/'Cargo.toml').write_text(manifest)
        env=dict(os.environ,CARGO_TARGET_DIR=str(ROOT/'target/thread-api-host'))
        for name,body,expected in CASES:
            (directory/'src/lib.rs').write_text('#![no_std]\nextern crate alloc;\npub fn probe(){'+body+'}\n')
            result=subprocess.run(['cargo','check','--offline','--target','x86_64-unknown-linux-gnu'],
                cwd=directory,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,timeout=120)
            if expected is None:
                assert result.returncode==0,result.stdout
            else:
                assert result.returncode!=0 and expected in result.stdout,result.stdout
            print(f'PASS: {name}',flush=True)

if __name__=='__main__':
    main()
