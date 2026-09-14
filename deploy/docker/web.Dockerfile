# syntax=docker/dockerfile:1
# Build context: repository root (see deploy/docker-compose.yml).

FROM oven/bun:1.3 AS deps
WORKDIR /app
COPY apps/web/package.json apps/web/bun.lock ./
RUN bun install --frozen-lockfile

FROM oven/bun:1.3 AS build
WORKDIR /app
COPY --from=deps /app/node_modules ./node_modules
COPY apps/web/ .
RUN bun run build

FROM oven/bun:1.3 AS runtime
WORKDIR /app
ENV NODE_ENV=production \
    PORT=3000 \
    HOST=0.0.0.0
COPY --from=build /app/build ./build
COPY --from=build /app/node_modules ./node_modules
COPY --from=build /app/package.json ./package.json
EXPOSE 3000
# `bun build` is Bun's bundler command, so name the entrypoint explicitly.
CMD ["bun", "build/index.js"]
