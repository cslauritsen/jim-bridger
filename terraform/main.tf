provider "aws" {
  region = var.region
}

terraform {
  backend "s3" {
    bucket         = "planetlauritsen-tf"
    key            = "ses_handler/terraform.tfstate"
    region         = "us-east-2"
  }
}

resource "aws_sqs_queue" "main" {
  name = "jim-bridger-queue"
  redrive_policy = jsonencode({
    deadLetterTargetArn = aws_sqs_queue.dlq.arn
    maxReceiveCount    = 5
  })
}

resource "aws_sqs_queue" "dlq" {
  name = "jim-bridger-dlq"
}

resource "aws_iam_role" "bridger_exec_role" {
  name = "jim_bridger_exec_role"
  assume_role_policy = jsonencode({
    Version = "2012-10-17",
    Statement = [{
      Action    = "sts:AssumeRole",
      Effect    = "Allow",
      Principal = {
        Service = "ec2.amazonaws.com"
      }
    }]
  })
}

resource "aws_iam_role_policy" "bridger_policy" {
  role = aws_iam_role.bridger_exec_role.id
  policy = jsonencode({
    Version = "2012-10-17",
    Statement = [
      {
        Effect = "Allow",
        Action = [
          "sqs:ReceiveMessage",
          "sqs:DeleteMessage",
          "sqs:GetQueueAttributes",
          "sqs:SendMessage"
        ],
        Resource = [aws_sqs_queue.main.arn, aws_sqs_queue.dlq.arn]
      },
      {
        Effect = "Allow",
        Action = [
          "s3:GetObject",
          "s3:DeleteObject"
        ],
        Resource = "${var.s3_bucket_arn}/*"
      },
      {
        Effect = "Allow",
        Action = [
          "secretsmanager:GetSecretValue"
        ],
        Resource = "${var.bridge_secret_arn}"
      },
    ]
  })
}

resource "aws_sqs_queue_policy" "main" {
  queue_url = aws_sqs_queue.main.id
  policy = jsonencode({
    Version = "2012-10-17",
    Statement = [
      {
        Effect = "Allow",
        Principal = { Service = "s3.amazonaws.com" },
        Action = "sqs:SendMessage",
        Resource = aws_sqs_queue.main.arn,
        Condition = {
          ArnEquals = {
            "aws:SourceArn" = var.s3_bucket_arn
          }
        }
      }
    ]
  })
}

resource "aws_s3_bucket_notification" "bucket_notify" {
  bucket = var.s3_bucket_name
  queue {
    queue_arn     = aws_sqs_queue.main.arn
    events        = ["s3:ObjectCreated:*"]
  }
  depends_on = [aws_sqs_queue_policy.main]
}

resource "aws_iam_user" "jim_bridger" {
  name = "jim-bridger-user"
}

resource "aws_iam_user_policy" "jim_bridger_policy" {
  name = "jim-bridger-user-policy"
  user = aws_iam_user.jim_bridger.name
  policy = jsonencode({
    Version = "2012-10-17",
    Statement = [
      {
        Effect = "Allow",
        Action = [
          "sqs:ReceiveMessage",
          "sqs:DeleteMessage",
          "sqs:GetQueueAttributes",
          "sqs:SendMessage"
        ],
        Resource = [aws_sqs_queue.main.arn, aws_sqs_queue.dlq.arn]
      },
      {
        Effect = "Allow",
        Action = [
          "s3:GetObject",
          "s3:DeleteObject",
          "s3:ListBucket"
        ],
        Resource = "${var.s3_bucket_arn}/*"
      },
      {
        Effect = "Allow",
        Action = [
          "secretsmanager:GetSecretValue"
        ],
        Resource = "${var.bridge_secret_arn}"
      },
    ]
  })
}

resource "aws_iam_access_key" "jim_bridger" {
  user = aws_iam_user.jim_bridger.name
}
