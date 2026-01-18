SQS Refactor

# Current State
Jim Bridger is a simple HTTP server that runs alongside Apache James in my k3s cluster. It listends for incoming HTTP 
POST requests that are expected to contain a rfc822 formatted email message in the body. Upon receiving such a request,
Jim Bridger parses the email message and extracts recipient information and then opens an SMTP cconnection to the
Apache James server to relay the email message to its intended recipients.

## Problems with Current State
### Availability Coupling
Jim Bridger is triggered by a Lambda function, which is triggered by the arrival of a new email in an S3 bucket. 
If jim bridger is down or James is down, the lambda function has no way to retry the delivery of the email message, and
manual intervention is required to ensure the email is delivered.
### Scalability Limitations
Jim Bridger is a single instance HTTP server. If the volume of incoming email messages increases, Jim Bridger may become a bottleneck,
leading to delays in email delivery.

### Pre-Shared Secrets
Jim Bridger requires a pre-shared secret to allow the Lambda function to authenticate its requests. This secret must 
be securely managed and rotated, adding operational overhead, and a small cost to use AWS secrets. It also is tedious to rotate.

# Proposed Refactor
To address the above problems, I propose refactoring the email relay system to use AWS Simple Queue Service (SQS).
The pseudo code description follows:
  SES places incoming message into s3 (established behavior)
  S3 ObjectCreated event triggers SQS message containing path to object in s3
  Jim bridger loops:
     establish sqs connection with STS-based auth
     long poll SQS for messages:
        For each SQS message, Jim Bridger:
            1. retrieves the email message from S3 using the path provided in the SQS message
            2. parses the email message and relays it to Apache James using an SMTP connection
            3. deletes the SQS message upon successful delivery
            4. Deletes the S3 object after successful delivery to avoid storage bloat.
          If any errors occur in the above steps:, 
              The message is left in the SQS queue and is retried using exponential backoff.
              After a configurable number of failed attempts, JB moves the message to a dead-letter queue for further investigation.


## Deployment architecture
Jim Bridger will be deployed as a containerized application in the existing k3s cluster, we can leave the HTTP functionality
in place. We will modify the code to contain the SQS polling logic described above. The existing Lambda function can be decommissioned.
## Question : should it run as a kubernetes cron job, or as a long-running deployment?
## Terraform 
Terraform will maange the SQS queue, DLQ, add the SQS message trigger on the s3 bucket, and manage any necessary 
IAM roles and policies for secure access to SQS and S3.