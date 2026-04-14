FROM node:20-alpine

# openssh-client enables POST /api/workflows/dispatch to invoke minion on the host
# (Epic 5 Story 5.1). Set MINION_DISPATCH_SSH_HOST=root@<host> in docker-compose
# and mount host keys read-only.
RUN apk add --no-cache openssh-client

WORKDIR /app

COPY package.json ./
RUN npm install

COPY tsconfig.json ./
COPY src/ ./src/
COPY migrations/ ./migrations/

CMD ["npx", "tsx", "watch", "src/index.ts"]
