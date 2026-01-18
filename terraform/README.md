# Build
Run the build script:

    ./build.sh

# Deploy
Initialize and apply the terraform

    terraform init
    export AWS_PROFILE=chad-admin
    terraform apply -var="s3_bucket_name=inmail-planetlauritsen" \
                -var="s3_bucket_arn=arn:aws:s3:::inmail-planetlauritsen" \
                -var="bridge_url=https://jim-bridger.home.planetlauritsen.com/incoming" \
                -var="bridge_secret=$(op read "op://Private/y26yrloe232cpc5cngcmjz35ty/password")"