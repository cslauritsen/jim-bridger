variable "region" {
  default = "us-east-2"
}

variable "s3_bucket_name" {
  description = "The name of the S3 bucket receiving SES mail"
  type        = string
}

variable "s3_bucket_arn" {
  description = "The ARN of the S3 bucket"
  type        = string
}

variable "bridge_url" {
  description = "The URL of your HTTP-to-SMTP/LMTP bridge"
  type        = string
}

variable "bridge_secret" {
  description = "The shared secret for authenticating to the bridge"
  type        = string
}
