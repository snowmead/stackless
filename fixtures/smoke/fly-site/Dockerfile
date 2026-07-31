FROM python:3.12-alpine
WORKDIR /srv
COPY index.html .
EXPOSE 8080
CMD ["python", "-m", "http.server", "8080", "--bind", "0.0.0.0"]
