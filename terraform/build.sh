#!/bin/bash
set -e
cd $(dirname $0)
rm -fr lambda_build lambda.zip
cp -r lambda lambda_build
cd lambda_build
python3 -m pip install -r requirements.txt -t .
zip -r ../lambda.zip ./*