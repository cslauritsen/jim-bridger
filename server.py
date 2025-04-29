from flask import Flask, request, abort
import aiosmtplib
import asyncio
import os
import email
from email import policy
import logging

import json

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
logger.setLevel(logging.INFO)

# Create a stream handler
handler = logging.StreamHandler()
handler.setFormatter(JsonFormatter())
# Add the handler to the logger
logger.addHandler(handler)
app = Flask(__name__)

SMTP_HOST = os.environ.get('SMTP_HOST', 'localhost')
SMTP_PORT = int(os.environ.get('SMTP_PORT', 25))
MAIL_SECRET = os.environ['PRE_SHARED_SECRET']

loop = asyncio.get_event_loop()

@app.route('/health', methods=['GET'])
def health_check():
    return "OK", 200


@app.route('/incoming', methods=['POST'])
def incoming_email():
    auth_header = request.headers.get('Authorization')
    if auth_header != f"Bearer {MAIL_SECRET}":
        abort(403)

    raw_email = request.data

    # Parse email headers
    try:
        parsed_email = email.message_from_bytes(raw_email, policy=policy.default)
        sender = parsed_email.get('From')
        logger.debug(f"From: {sender}")
        recipients = parsed_email.get_all('To', [])
        logger.debug(f"To: {recipients}")

        # Normalize recipients into list
        if isinstance(recipients, str):
            recipients = [recipients]

        # Optional: include Cc recipients too
        cc_recipients = parsed_email.get_all('Cc', [])
        if cc_recipients:
            if isinstance(cc_recipients, str):
                cc_recipients = [cc_recipients]
            recipients.extend(cc_recipients)

        if not sender or not recipients:
            logger.error("No sender or recipients")
            abort(400, description="Missing sender or recipient information")

        logger.info(f"Sending email from {sender} to {recipients}")

        async def send_email():
            await aiosmtplib.send(
                parsed_email,
                hostname=SMTP_HOST,
                # port=SMTP_PORT,
                # username=os.environ.get('SMTP_USERNAME'),
                # password=os.environ.get('SMTP_PASSWORD'),
                # start_tls=True,
            )

        loop.run_until_complete(send_email())

        return "Email accepted", 200
    except Exception as e:
        logger.info(f"Error parsing email: {e}")
        abort(400, description=f'failed to parse or send message')


if __name__ == '__main__':
    app.run(host='0.0.0.0', port=8080)
