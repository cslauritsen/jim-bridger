import os
os.environ['PRE_SHARED_SECRET'] = 'dummy-secret'

import json
import pytest
from unittest.mock import patch, MagicMock

import server

# Load the sample S3 notification event
with open('testdata/s3notif.json') as f:
    S3_EVENT = json.load(f)

@pytest.fixture
def s3_event():
    return S3_EVENT

@pytest.fixture
def sqs_message(s3_event):
    return {
        'Body': json.dumps(s3_event),
        'ReceiptHandle': 'dummy-receipt',
        'Attributes': {'ApproximateReceiveCount': '1'}
    }

def test_parse_s3_event_format(sqs_message):
    event = json.loads(sqs_message['Body'])
    records = event.get('Records', [])
    assert len(records) == 1
    rec = records[0]
    s3_info = rec.get('s3', {})
    s3_bucket = s3_info.get('bucket', {}).get('name')
    s3_key = s3_info.get('object', {}).get('key')
    assert s3_bucket == 'inmail-planetlauritsen'
    assert s3_key == 'incoming/2nmv0tmvbj0a4ubaq70vre42du72itdprtgvrgo1'

def test_sqs_poller_handles_s3_event(monkeypatch, sqs_message):
    # Patch boto3 clients
    mock_sqs = MagicMock()
    mock_sqs.receive_message.return_value = {'Messages': [sqs_message]}
    mock_sqs.delete_message.return_value = None
    mock_s3 = MagicMock()
    mock_s3.get_object.return_value = {'Body': MagicMock(read=lambda: b'raw email bytes')}
    mock_s3.delete_object.return_value = None
    # Patch process_email_message to always succeed
    monkeypatch.setattr(server, 'boto3', MagicMock(client=lambda service, region_name=None: mock_sqs if service=='sqs' else mock_s3))
    monkeypatch.setattr(server, 'process_email_message', lambda parsed_email: (True, ['to@example.com'], None))
    # Patch email.message_from_bytes to avoid parsing
    monkeypatch.setattr(server.email, 'message_from_bytes', lambda raw, policy=None: MagicMock())
    # Patch logger to avoid clutter
    monkeypatch.setattr(server, 'logger', MagicMock())
    # Patch time.sleep to avoid delays
    monkeypatch.setattr(server, 'time', 'sleep', lambda x: None)
    # Set env vars
    monkeypatch.setenv('SQS_QUEUE_URL', 'dummy')
    monkeypatch.setenv('ENABLE_SQS_POLL', 'true')
    monkeypatch.setenv('S3_BUCKET_NAME', 'inmail-planetlauritsen')
    # Run one poll iteration
    poller = server.start_sqs_poller
    poller()  # Should process the message without error

def xtest_sqs_poller_sends_to_dlq_on_failure(monkeypatch, sqs_message):
    # Simulate max retries reached
    sqs_message['Attributes']['ApproximateReceiveCount'] = '5'
    mock_sqs = MagicMock()
    mock_sqs.receive_message.return_value = {'Messages': [sqs_message]}
    mock_sqs.delete_message.return_value = None
    mock_sqs.send_message.return_value = None
    mock_s3 = MagicMock()
    mock_s3.get_object.return_value = {'Body': MagicMock(read=lambda: b'raw email bytes')}
    mock_s3.delete_object.return_value = None
    # Patch process_email_message to always fail
    monkeypatch.setattr(server, 'boto3', MagicMock(client=lambda service, region_name=None: mock_sqs if service=='sqs' else mock_s3))
    monkeypatch.setattr(server, 'process_email_message', lambda parsed_email: (False, [], 'SMTP error'))
    monkeypatch.setattr(server.email, 'message_from_bytes', lambda raw, policy=None: MagicMock())
    monkeypatch.setattr(server, 'logger', MagicMock())
    monkeypatch.setattr(server, 'time', 'sleep', lambda x: None)
    monkeypatch.setenv('SQS_QUEUE_URL', 'dummy')
    monkeypatch.setenv('SQS_DLQ_URL', 'dlq-url')
    monkeypatch.setenv('ENABLE_SQS_POLL', 'true')
    monkeypatch.setenv('S3_BUCKET_NAME', 'inmail-planetlauritsen')
    # Run one poll iteration
    poller = server.start_sqs_poller
    poller()
    # Check that send_message to DLQ was called
    assert mock_sqs.send_message.called, "DLQ send_message should be called on max retries"
    assert mock_sqs.delete_message.called, "delete_message should be called after DLQ send"
