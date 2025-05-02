import os
import boto3
import requests

S3 = boto3.client('s3')

BRIDGE_URL = os.environ['BRIDGE_URL']  # e.g., https://mail-ingest.home.planetlauritsen.com/incoming
BRIDGE_SECRET = os.environ['BRIDGE_SECRET']  # your shared auth token

def lambda_handler(event, context):
    print(f"Received event: {event}")

    for record in event['Records']:
        s3_bucket = record['s3']['bucket']['name']
        s3_object_key = record['s3']['object']['key']

        # Get the email content from S3
        response = S3.get_object(Bucket=s3_bucket, Key=s3_object_key)
        email_content = response['Body'].read()

        # Send the email content to your home mail server
        headers = {
            'Authorization': f"Bearer {BRIDGE_SECRET}",
            'Content-Type': 'application/octet-stream'
        }

        post_response = requests.post(
            BRIDGE_URL,
            headers=headers,
            data=email_content
        )

        print(f"POST response: {post_response.status_code} - {post_response.text}")
        if post_response.status_code == 400:
            print("Bad request: check the email content")
            S3.delete_object(Bucket=s3_bucket, Key=s3_object_key)
            print(f"{s3_object_key} deleted")
            return {'status': 'done'}

        if post_response.status_code >= 200 and post_response.status_code < 300:
            print("Email sent successfully")
            S3.delete_object(Bucket=s3_bucket, Key=s3_object_key)
            print(f"{s3_object_key} deleted")

        post_response.raise_for_status()

    return {'status': 'done'}
