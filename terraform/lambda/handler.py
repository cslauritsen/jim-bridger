import os
import boto3
import requests
from secret import get_secret

S3 = boto3.client('s3')

BRIDGE_URL = os.environ['BRIDGE_URL']  # e.g., https://mail-ingest.home.planetlauritsen.com/incoming


def lambda_handler(event, context):
    print(f"Received event: {event}")
    secret = get_secret()

    for record in event['Records']:
        s3_bucket = record['s3']['bucket']['name']
        s3_object_key = record['s3']['object']['key']

        # Get the email content from S3
        response = S3.get_object(Bucket=s3_bucket, Key=s3_object_key)
        email_content = response['Body'].read()

        # Send the email content to your home mail server
        headers = {
            'Authorization': f"Bearer {secret}",
            'Content-Type': 'application/octet-stream'
        }

        post_response = requests.post(
            BRIDGE_URL,
            headers=headers,
            data=email_content
        )

        print(f"POST response: {post_response.status_code} - {post_response.text}")

        if 200 <= post_response.status_code < 300:
            print("Email sent successfully")
            S3.delete_object(Bucket=s3_bucket, Key=s3_object_key)
            print(f"{s3_object_key} deleted")
            return {'status': 'done'}
        else:
            print(f"Forwarding failed HTTP {post_response.status_code} check payload format")
            print(f"{s3_object_key} left in situ")
            return {'status': 'done'}


    return {'status': 'done'}