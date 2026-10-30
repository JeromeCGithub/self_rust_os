#!/usr/bin/env python3
"""Create disposable TLS test authorities and certificates with OpenSSL."""
from pathlib import Path
import subprocess
import sys

def create(root):
    root=Path(root);root.mkdir(parents=True,exist_ok=True)
    def run(*args):subprocess.run(['openssl',*map(str,args)],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    run('req','-x509','-newkey','ec','-pkeyopt','ec_paramgen_curve:P-256','-nodes','-keyout',root/'ca.key','-out',root/'ca.pem','-days','3650','-subj','/CN=RustOS TLS Test CA','-addext','basicConstraints=critical,CA:TRUE','-addext','keyUsage=critical,keyCertSign,cRLSign')
    (root/'index').write_text('');(root/'serial').write_text('01\n');(root/'newcerts').mkdir(exist_ok=True)
    (root/'ca.cnf').write_text(f'''[ca]
default_ca=test
[test]
database={root}/index
serial={root}/serial
new_certs_dir={root}/newcerts
certificate={root}/ca.pem
private_key={root}/ca.key
default_md=sha256
policy=policy
unique_subject=no
[policy]
commonName=supplied
[server]
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature
extendedKeyUsage=serverAuth
subjectAltName=DNS:fixture.test,IP:10.0.2.2
''')
    for name,rsa,start,end in [('valid',False,'20200101000000Z','20400101000000Z'),('expired',False,'20000101000000Z','20010101000000Z'),('future',False,'20500101000000Z','20600101000000Z'),('rsa',True,'20200101000000Z','20400101000000Z')]:
        run('req','-new','-newkey',*(('rsa:2048',) if rsa else ('ec','-pkeyopt','ec_paramgen_curve:P-256')),'-nodes','-keyout',root/(name+'.key'),'-out',root/(name+'.csr'),'-subj','/CN=fixture.test')
        run('ca','-batch','-config',root/'ca.cnf','-extensions','server','-startdate',start,'-enddate',end,'-in',root/(name+'.csr'),'-out',root/(name+'.pem'),'-notext')
        run('x509','-in',root/(name+'.pem'),'-outform','DER','-out',root/(name+'.der'))
        run('pkcs8','-topk8','-nocrypt','-in',root/(name+'.key'),'-outform','DER','-out',root/(name+'-key.der'))
    run('x509','-in',root/'ca.pem','-outform','DER','-out',root/'ca.der')
    return root

if __name__=='__main__':create(Path(sys.argv[1]).resolve())
