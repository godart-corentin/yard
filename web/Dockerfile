FROM python:3.13-alpine

WORKDIR /opt/yard

COPY server/yard_web.py /opt/yard/server.py
COPY static /opt/yard/static

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    YARD_WEB_HOST=0.0.0.0 \
    YARD_WEB_PORT=8088 \
    YARD_PROJECTS_DIR=/etc/yard/projects \
    YARD_STATE_DIR=/var/lib/yard \
    YARD_WEB_STATIC=/opt/yard/static

CMD ["python3", "/opt/yard/server.py"]
