FROM rust:1.88-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --locked --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends poppler-utils tesseract-ocr tesseract-ocr-eng tesseract-ocr-hin procps ca-certificates curl && rm -rf /var/lib/apt/lists/* && useradd --system --create-home extractor
COPY --from=build /app/target/release/doc-extractor /usr/local/bin/doc-extractor
WORKDIR /home/extractor
USER extractor
EXPOSE 8787
# Set DOC_TOKENS for network bindings. Loopback remains the secure default.
ENTRYPOINT ["doc-extractor"]
CMD ["serve", "--data-dir", "/home/extractor/data"]
