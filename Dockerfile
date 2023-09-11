FROM node:20 as build
ENV CI true

ENV PORT=8080
WORKDIR /src

RUN corepack enable && corepack prepare --all
COPY . .

WORKDIR /src/packages/playground

RUN pnpm install
RUN pnpm run build

FROM nginx:alpine as runtime
ENV CI true

COPY --from=build /src/dist /usr/share/nginx/html/static
RUN echo "\
    server { \
        listen 8080; \
        root /usr/share/nginx/html/static; \
        absolute_redirect off; \
        location / { \
            try_files \$uri /index.html; \
        } \
    }"> /etc/nginx/conf.d/default.conf

EXPOSE 8080
