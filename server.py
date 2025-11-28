from flask import Flask, request, abort
import aiosmtplib
import asyncio
import os
import email
from email import policy
import logging
import json
from prometheus_client import Counter, generate_latest, CONTENT_TYPE_LATEST
from email.utils import parseaddr, getaddresses
from email.headerregistry import Address

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
MAIL_SECRET = os.environ['PRE_SHARED_SECRET']
DEFAULT_SENDER_DOMAIN = os.environ.get('DEFAULT_SENDER_DOMAIN', 'planetlauritsen.com')

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
        FAILURE_METRIC.inc()
        abort(401, description="Unauthorized")

    raw_email = request.data

    try:
        parsed_email = email.message_from_bytes(raw_email, policy=policy.default)

        # ---- Extract original sender from header ----
        original_from = parsed_email.get('From', '')
        _, original_sender = parseaddr(original_from)

        # ---- Extract recipients (To + Cc) correctly ----
        to_addresses = parsed_email.get_all('To', [])
        cc_addresses = parsed_email.get_all('Cc', [])

        all_recipients = getaddresses(to_addresses + cc_addresses)
        # rewrite recips csl4jc@gmail.com or csla@hey.com with chad@planetlauritsen.com

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
            # logger.error("No recipients found in message")
            # FAILURE_METRIC.inc()
            # abort(400, description="Missing recipients")

        # ---- Force envelope sender for SMTP ----
        envelope_sender = FORWARDER_ADDRESS

        # ---- Ensure original sender is preserved in headers ----
        # Before setting Reply-To, check if it exists
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

        loop.run_until_complete(send_email())

        SUCCESS_METRIC.inc()
        return f"Email accepted for {','.join(recipients)}", 200

    except Exception as e:
        logger.exception(f"Error processing incoming email: {e}")
        FAILURE_METRIC.inc()
        abort(400, description="Failed to parse or send message")

if __name__ == '__main__':
    app.run(host='0.0.0.0', port=8080)