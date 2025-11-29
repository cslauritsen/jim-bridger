#!/usr/bin/env python3
import os
import shutil
import subprocess
import sys
from pathlib import Path

def main():
    script_dir = Path(__file__).parent.resolve()
    os.chdir(script_dir)

    # Remove old build artifacts
    shutil.rmtree('lambda_build', ignore_errors=True)
    if Path('lambda.zip').exists():
        os.remove('lambda.zip')

    # Copy lambda source
    shutil.copytree('lambda', 'lambda_build')

    # Install requirements
    subprocess.check_call([
        sys.executable, '-m', 'pip', 'install',
        '-r', './lambda/requirements.txt', '-t', 'lambda_build'
    ])

    # Zip the build directory
    shutil.make_archive('lambda', 'zip', 'lambda_build')

if __name__ == '__main__':
    main()
