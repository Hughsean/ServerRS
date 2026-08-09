#!/bin/sh
set -eu

mysql_exec() {
  MYSQL_PWD="$QQBOT_MYSQL_PASSWORD" mysql \
    --protocol=tcp \
    --host=qqbot-mysql \
    --user="$QQBOT_MYSQL_USER" \
    "$QQBOT_MYSQL_DATABASE" "$@"
}

mysql_exec --execute="
  CREATE TABLE IF NOT EXISTS qqbot_schema_migrations (
    migration_name VARCHAR(255) CHARACTER SET ascii COLLATE ascii_bin NOT NULL PRIMARY KEY,
    applied_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
  ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
"

for migration in /migrations/*.sql; do
  [ -f "$migration" ] || continue
  migration_name=$(basename "$migration")
  escaped_name=$(printf '%s' "$migration_name" | sed "s/'/''/g")
  applied=$(mysql_exec --batch --skip-column-names \
    --execute="SELECT COUNT(*) FROM qqbot_schema_migrations WHERE migration_name = '$escaped_name'")
  if [ "$applied" = "0" ]; then
    mysql_exec < "$migration"
    mysql_exec --execute="INSERT INTO qqbot_schema_migrations (migration_name) VALUES ('$escaped_name')"
  fi
done
