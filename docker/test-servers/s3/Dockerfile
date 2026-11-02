# A throwaway S3-compatible server for testing Dual Pal: the moto emulator,
# with one empty bucket. Keys: access key "test", secret "test-secret".
FROM python:3.12-alpine

RUN pip install --no-cache-dir "moto[server]==5.1.*" boto3

ENV BUCKET=dualpal-test
EXPOSE 9000
CMD ["sh", "-c", "moto_server -H 0.0.0.0 -p 9000 & \
  for i in $(seq 1 30); do wget -q -O /dev/null http://127.0.0.1:9000/ && break; sleep 1; done; \
  python -c \"import boto3,os; boto3.client('s3', endpoint_url='http://127.0.0.1:9000', aws_access_key_id='test', aws_secret_access_key='test-secret', region_name='us-east-1').create_bucket(Bucket=os.environ['BUCKET'])\" && \
  echo \"bucket $BUCKET ready\"; wait"]
