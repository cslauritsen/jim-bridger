import asyncio
import email
import json
import logging
import os
from email import policy
from email.utils import parseaddr, getaddresses

import aiosmtplib
from flask import Flask, request, abort
from prometheus_client import Counter, generate_latest, CONTENT_TYPE_LATEST
import threading
import boto3
import time
from botocore.exceptions import ClientError

DEFAULT_RECIPIENT = 'chad@planetlauritsen.com'

FORWARDER_ADDRESS = "ses-forwarder@planetlauritsen.com"

class JsonFormatter(logging.Formatter):
    def format(self, record):
        log_record = {
            "level": record.levelname,
            "message": record.getMessage(),
            "timestamp": self.formatTime(record, self.datefmt),
            "name": record.name,
            "module": record.module,
            "funcName": record.funcName,
            "lineNo": record.lineno,
        }
        return json.dumps(log_record)

# Configure the logger
logger = logging.root

# Create a stream handler
handler = logging.StreamHandler()
handler.setFormatter(JsonFormatter())
logger.addHandler(handler)
app = Flask(__name__)

levels = {
    'CRITICAL': logging.CRITICAL,
    'ERROR': logging.ERROR,
    'WARNING': logging.WARNING,
    'INFO': logging.INFO,
    'DEBUG': logging.DEBUG,
}

# use envvar to configure root logger
root_log_level = os.environ.get('LOG_LEVEL_ROOT', 'INFO').upper()
logger.setLevel(levels.get(root_log_level, logging.INFO))

# configure Flask's internal logging set to WARNING to quiet down
flask_log_level = os.environ.get('LOG_LEVEL_FLASK', 'INFO').upper()
app.logger.setLevel(levels.get(flask_log_level, logging.WARNING))
# configure Werkzeug request logging (set to ERROR to quiet down)
wz_level = os.environ.get('LOG_LEVEL_WERKZEUG', 'INFO').upper()
logging.getLogger('werkzeug').setLevel(levels.get(wz_level, logging.ERROR))

# Prometheus metrics
SUCCESS_METRIC = Counter('email_bridge_success', 'Number of successfully bridged emails')
FAILURE_METRIC = Counter('email_bridge_failure', 'Number of failed email bridging attempts')
HEALTHCHECK_METRIC = Counter('email_bridge_healthcheck', 'Number of health check requests')
AUTH_FAILED_METRIC = Counter('auth_failure', 'Number of failed authorization attempts')
SCRAPE_METRIC = Counter('scrapes', 'Number of scrapes')

SMTP_HOST = os.environ.get('SMTP_HOST', 'localhost')
SMTP_PORT = int(os.environ.get('SMTP_PORT', 25))
SMTP_POLICY = policy.SMTPUTF8.clone(max_line_length=1024*1024)
MAIL_SECRET = os.environ['PRE_SHARED_SECRET']
DEFAULT_SENDER_DOMAIN = os.environ.get('DEFAULT_SENDER_DOMAIN', 'planetlauritsen.com')
sqs_failures = []

loop = asyncio.get_event_loop()
@app.route('/health', methods=['GET'])
def health_check():
    logger.debug('msg=health_check ip=%s ua="%s"', request.remote_addr, request.user_agent)
    HEALTHCHECK_METRIC.inc()
    if len(sqs_failures) > 5:
        logger.error("SQS failures exceed threshold: %s", sqs_failures)
        # allow crashloopbackoff so we don't spam SQS if misconfigured
        return "SQS failures exceed threshold", 418
    return "OK", 200

@app.route('/metrics', methods=['GET'])
def metrics():
    SCRAPE_METRIC.inc()
    logger.debug('msg=scrape ip=%s ua="%s"', request.remote_addr, request.user_agent)
    return generate_latest(), 200, {'Content-Type': CONTENT_TYPE_LATEST}

def process_email_message(parsed_email):
    try:
        original_from = parsed_email.get('From', '')
        _, original_sender = parseaddr(original_from)
        to_addresses = parsed_email.get_all('To', [])
        cc_addresses = parsed_email.get_all('Cc', [])
        bcc_addresses = parsed_email.get_all('Bcc', [])
        all_recipients = getaddresses(to_addresses + cc_addresses + bcc_addresses)
        x_forwarded_to = parsed_email.get_all('X-Forwarded-To', [])
        recipients = []
        if len(x_forwarded_to) > 0:
            logger.info(f"X-Forwarded-For detected: {x_forwarded_to}")
            recipients.append(x_forwarded_to[0])
        else:
            for _, addr in all_recipients:
                recipients.append(addr)
        if not recipients:
            recipients.append(DEFAULT_RECIPIENT)
        envelope_sender = FORWARDER_ADDRESS
        # Ensure original sender is preserved in headers
        if original_sender:
            parsed_email.replace_header("From", original_from)
            if "Reply-To" in parsed_email:
                parsed_email.replace_header("Reply-To", original_sender)
            else:
                parsed_email["Reply-To"] = original_sender
        else:
            parsed_email.replace_header("From", FORWARDER_ADDRESS)
        logger.info(f"Forwarding message: envelope-from={envelope_sender}, recipients={recipients}")
        start_tls = os.environ.get('SMTP_STARTTLS', 'False').lower() == 'true'
        async def send_email():
            logger.debug("attempting SMTP forwarding")
            try:
                await aiosmtplib.send(
                    parsed_email,
                    sender=envelope_sender,
                    recipients=recipients if len(recipients) > 0 else [DEFAULT_RECIPIENT],
                    hostname=SMTP_HOST,
                    port=SMTP_PORT,
                    username=os.environ.get('SMTP_USERNAME'),
                    password=os.environ.get('SMTP_PASSWORD'),
                    start_tls=start_tls,
                )
                logger.debug(f"forwarded message via SMTP to {recipients}")
                return True, None
            except aiosmtplib.SMTPResponseException as smtp_exc:
                logger.error(f"SMTP error: {smtp_exc.code} {smtp_exc.message}")
                # 5xx is permanent, 4xx is transient
                if 500 <= smtp_exc.code < 600:
                    return False, True  # permanent failure
                else:
                    return False, False  # transient failure
            except Exception as e:
                logger.exception(f"Unexpected error in SMTP send: {e}")
                return False, False  # treat as transient
        send_success, permanent_fail = loop.run_until_complete(send_email())
        if send_success:
            return True, recipients, None
        else:
            return False, [], 'permanent' if permanent_fail else 'transient'
    except Exception as e:
        logger.exception(f"Error processing email: {e}")
        return False, [], 'transient'

@app.route('/incoming', methods=['POST'])
def incoming_email():
    auth_header = request.headers.get('Authorization')
    if auth_header != f"Bearer {MAIL_SECRET}":
        AUTH_FAILED_METRIC.inc()
        FAILURE_METRIC.inc()
        abort(401, description="Unauthorized")
    raw_email = request.data
    try:
        parsed_email = email.message_from_bytes(raw_email, policy=SMTP_POLICY)
        success, recipients, error = process_email_message(parsed_email)
        if success:
            SUCCESS_METRIC.inc()
            return f"Email accepted for {','.join(recipients)}", 200
        else:
            FAILURE_METRIC.inc()
            abort(400, description=f"Failed to parse or send message: {error}")
    except Exception as e:
        logger.exception(f"Error processing incoming email: {e}")
        FAILURE_METRIC.inc()
        abort(400, description="Failed to parse or send message")

def start_sqs_poller():
    SQS_QUEUE_URL = os.environ.get('SQS_QUEUE_URL')
    DLQ_URL = os.environ.get('SQS_DLQ_URL')
    S3_BUCKET = os.environ.get('S3_BUCKET_NAME')
    AWS_REGION = os.environ.get('AWS_REGION', 'us-east-2')
    MAX_RETRIES = int(os.environ.get('SQS_MAX_RETRIES', 5))
    POLL_WAIT = int(os.environ.get('SQS_POLL_WAIT', 20))
    ENABLE_SQS = os.environ.get('ENABLE_SQS_POLL', 'false').lower() == 'true'
    if not ENABLE_SQS or not SQS_QUEUE_URL:
        logger.info("SQS polling not enabled or missing SQS_QUEUE_URL")
        return

    sqs = boto3.client('sqs', region_name=AWS_REGION)
    s3 = boto3.client('s3', region_name=AWS_REGION)

    def poll():
        logger.info("Starting SQS polling loop")
        while True:
            try:
                resp = sqs.receive_message(
                    QueueUrl=SQS_QUEUE_URL,
                    MaxNumberOfMessages=1,
                    WaitTimeSeconds=POLL_WAIT,
                    VisibilityTimeout=60,
                    MessageAttributeNames=['All'],
                    AttributeNames=['All']
                )
                messages = resp.get('Messages', [])
                if not messages or len(messages) == 0:
                    logger.debug(f"No messages found in SQS response")
                    continue
                for msg in messages:
                    receipt_handle = msg['ReceiptHandle']
                    body = msg['Body']
                    try:
                        event = json.loads(body)
                        # S3 event notification format
                        records = event.get('Records', [])
                        if not records:
                            logger.error(f"No Records found in SQS message body: {body}")
                            FAILURE_METRIC.inc()
                            sqs.delete_message(QueueUrl=SQS_QUEUE_URL, ReceiptHandle=receipt_handle)
                            continue
                        all_success = True
                        for rec in records:
                            s3_info = rec.get('s3', {})
                            s3_bucket = s3_info.get('bucket', {}).get('name', S3_BUCKET)
                            s3_key = s3_info.get('object', {}).get('key')
                            if not s3_bucket or not s3_key:
                                logger.error(f"Missing s3 bucket or key in SQS record: {json.dumps(rec)}")
                                FAILURE_METRIC.inc()
                                all_success = False
                                continue
                            s3url = f"s3://{s3_bucket}/{s3_key}"
                            logger.info(f"Processing SQS record for S3 object: {s3url}")
                            try:
                                try:
                                    s3_obj = s3.get_object(Bucket=s3_bucket, Key=s3_key)
                                except ClientError as e:
                                    error_code = e.response.get('Error', {}).get('Code')
                                    if error_code == 'NoSuchKey':
                                        logger.warning(f"S3 object already deleted: {s3url}. Treating as success.")
                                        continue  # treat as success, don't fail or escalate
                                    else:
                                        raise
                                raw_email = s3_obj['Body'].read()
                                parsed_email = email.message_from_bytes(raw_email, policy=SMTP_POLICY)
                                success, recipients, error_type = process_email_message(parsed_email)
                                if success:
                                    s3.delete_object(Bucket=s3_bucket, Key=s3_key)
                                    logger.info(f"Successfully processed and deleted S3 object: {s3url}")
                                    SUCCESS_METRIC.inc()
                                else:
                                    logger.error(f"{s3url} Failed to process email from SQS record: {error_type}")
                                    FAILURE_METRIC.inc()
                                    if error_type == 'permanent':
                                        logger.warning(f"Permanent failure for {s3url}, deleting SQS message to avoid retry loop.")
                                        sqs.delete_message(QueueUrl=SQS_QUEUE_URL, ReceiptHandle=receipt_handle)
                                    else:
                                        all_success = False
                            except Exception as e:
                                logger.exception(f"Error processing SQS record: {e}")
                                FAILURE_METRIC.inc()
                                all_success = False
                        # Only delete SQS message if all records processed successfully
                        if all_success:
                            sqs.delete_message(QueueUrl=SQS_QUEUE_URL, ReceiptHandle=receipt_handle)
                            logger.debug(f"Successfully processed and deleted SQS message: {receipt_handle} with {len(records)} records")
                        else:
                            retry_count = int(msg.get('Attributes', {}).get('ApproximateReceiveCount', '1'))
                            if retry_count >= MAX_RETRIES and DLQ_URL:
                                logger.warning(f"Moving message to DLQ after {retry_count} attempts")
                                sqs.send_message(QueueUrl=DLQ_URL, MessageBody=body)
                                sqs.delete_message(QueueUrl=SQS_QUEUE_URL, ReceiptHandle=receipt_handle)
                    except Exception as e:
                        logger.exception(f"Error processing SQS message: {e}")
                        retry_count = int(msg.get('Attributes', {}).get('ApproximateReceiveCount', '1'))
                        if retry_count >= MAX_RETRIES and DLQ_URL:
                            logger.warning(f"Moving message to DLQ after {retry_count} attempts")
                            sqs.send_message(QueueUrl=DLQ_URL, MessageBody=body)
                            sqs.delete_message(QueueUrl=SQS_QUEUE_URL, ReceiptHandle=receipt_handle)
                        FAILURE_METRIC.inc()
                sqs_failures.clear()
            except Exception as e:
                # allow crashloopbackoff so we don't spam SQS if misconfigured
                sqs_failures.append(str(e))
                logger.exception(f"SQS polling loop error: {e}")
                time.sleep(10)
    t = threading.Thread(target=poll, daemon=True)
    t.start()

if __name__ == '__main__':
    start_sqs_poller()
    app.run(host='0.0.0.0', port=8080)