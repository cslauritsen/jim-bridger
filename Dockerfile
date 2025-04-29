# Use a small Python base image
FROM python:3.12-slim

# Set environment variables
ENV PYTHONDONTWRITEBYTECODE=1
ENV PYTHONUNBUFFERED=1

# Install required Python packages
WORKDIR /app

COPY requirements.txt /app/requirements.txt
RUN pip install --no-cache-dir -r requirements.txt

# Copy app source code
COPY *.py ./

# Expose the listening port
EXPOSE 8080
USER 1000
# Run the app
CMD ["python", "server.py"]
