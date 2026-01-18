output "sqs_queue_url" {
  value = aws_sqs_queue.main.id
}

output "sqs_dlq_url" {
  value = aws_sqs_queue.dlq.id
}

output "jim_bridger_access_key_id" {
  value = aws_iam_access_key.jim_bridger.id
  sensitive = true
}

output "jim_bridger_secret_access_key" {
  value = aws_iam_access_key.jim_bridger.secret
  sensitive = true
}

output "jim_bridger_iam_user_name" {
  value = aws_iam_user.jim_bridger.name
}

