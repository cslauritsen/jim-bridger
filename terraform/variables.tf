variable "region" {
  default = "us-east-2"
}

variable "s3_bucket_name" {
  description = "The name of the S3 bucket receiving SES mail"
  type        = string
  default     = "inmail-planetlauritsen"
}

variable "s3_bucket_arn" {
  description = "The ARN of the S3 bucket"
  type        = string
  default     = "arn:aws:s3:::inmail-planetlauritsen"
}

variable "bridge_url" {
  description = "The URL of your HTTP-to-SMTP/LMTP bridge"
  type        = string
  default     = "https://jim-bridger.home.planetlauritsen.com/incoming"
}

variable "bridge_secret_arn" {
  description = "The ARN of the Secrets Manager secret containing the bridge api_key"
  type        = string
  default     = "arn:aws:secretsmanager:us-east-2:111657657102:secret:prod/jim-bridger-MXpLCp"
}
