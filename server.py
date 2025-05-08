from flask import Flask, request, abort
import aiosmtplib
import asyncio
import os
import email
from email import policy
import logging
import json
from prometheus_client import Counter, generate_latest, CONTENT_TYPE_LATEST

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
MAIL_SECRET = os.environ['PRE_SHARED_SECRET']

loop = asyncio.get_event_loop()
@app.route('/health', methods=['GET'])
def health_check():
    logger.debug('msg=health_check ip=%s ua="%s"', request.remote_addr, request.user_agent)
    HEALTHCHECK_METRIC.inc()
    return "OK", 200

@app.route('/metrics', methods=['GET'])
def metrics():
    SCRAPE_METRIC.inc()
    logger.debug('msg=scrape ip=%s ua="%s"', request.remote_addr, request.user_agent)
    return generate_latest(), 200, {'Content-Type': CONTENT_TYPE_LATEST}

@app.route('/incoming', methods=['POST'])
def incoming_email():
    auth_header = request.headers.get('Authorization')
    if auth_header != f"Bearer {MAIL_SECRET}":
        AUTH_FAILED_METRIC.inc()
        FAILURE_METRIC.inc()  # Increment failure metric
        abort(403)

    raw_email = request.data

    try:
        parsed_email = email.message_from_bytes(raw_email, policy=policy.default)
        sender = parsed_email.get('From')
        logger.debug(f"From: {sender}")
        recipients = parsed_email.get_all('To', [])
        logger.debug(f"To: {recipients}")

        if isinstance(recipients, str):
            recipients = [recipients]

        cc_recipients = parsed_email.get_all('Cc', [])
        if cc_recipients:
            if isinstance(cc_recipients, str):
                cc_recipients = [cc_recipients]
            recipients.extend(cc_recipients)

        if not sender or not recipients:
            logger.error("No sender or recipients")
            FAILURE_METRIC.inc()  # Increment failure metric
            abort(400, description="Missing sender or recipient information")

        logger.info(f"Sending email from {sender} to {recipients}")

        start_tls = os.environ.get('SMTP_STARTTLS', 'False').lower() == 'true'
        async def send_email():
            await aiosmtplib.send(
                parsed_email,
                hostname=SMTP_HOST,
                port=SMTP_PORT,
                username=os.environ.get('SMTP_USERNAME', None),
                password=os.environ.get('SMTP_PASSWORD', None),
                start_tls=start_tls if start_tls else None,
            )

        loop.run_until_complete(send_email())
        SUCCESS_METRIC.inc()  # Increment success metric
        return "Email accepted", 200
    except Exception as e:
        logger.info(f"Error parsing email: {e}")
        FAILURE_METRIC.inc()  # Increment failure metric
        abort(400, description=f'failed to parse or send message')

if __name__ == '__main__':
    app.run(host='0.0.0.0', port=8080)