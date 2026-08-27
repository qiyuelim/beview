#!/usr/bin/env bash
# 开发阶段清库脚本：TRUNCATE 除白名单外所有业务表的数据（RESTART IDENTITY CASCADE）。
#
# 保留（白名单）：users（账号）、settings（LLM 配置/自定义提示词/简历显示偏好等 KV）、
#                _sqlx_migrations（迁移版本记录）。
#
# 用法:
#   ./scripts/reset_data.sh --list     # 只打印将清空/保留的表，不执行任何变更
#   ./scripts/reset_data.sh            # 交互确认后执行
#   ./scripts/reset_data.sh --yes      # 跳过确认直接执行（CI/脚本管道用）
#   DATABASE_URL=postgres://... ./scripts/reset_data.sh --yes
#
# 说明:
#   - DSN 解析同 backup.sh：优先 DATABASE_URL 环境变量，其次 server/config.toml 的 database_url；
#   - 表清单从 pg_catalog 动态枚举——后续迁移新增的表自动纳入清理范围，无需改脚本
#     （对照 export.rs 表清单漂移的教训）；新增需永久保留的表请改 KEEP_TABLES；
#   - RESTART IDENTITY 会把各表序列重置回 1，符合“开发干净态”预期；
#   - settings 整表保留（LLM api_key 密文依赖 server/.master_key，不受影响）。
set -euo pipefail
cd "$(dirname "$0")/.."

DB_URL="${DATABASE_URL:-$(sed -nE 's/^[[:space:]]*database_url[[:space:]]*=[[:space:]]*"([^"]*)".*/\1/p' server/config.toml 2>/dev/null || true)}"
if [ -z "$DB_URL" ]; then
  echo "未找到 database_url，请用 DATABASE_URL 环境变量指定" >&2
  exit 1
fi

command -v psql >/dev/null 2>&1 || { echo "需要 psql（postgresql-client）" >&2; exit 1; }

# 永久保留的白名单（勿清）
KEEP_TABLES="'users','settings','_sqlx_migrations'"

PSQL=(psql "$DB_URL" -X -At -v ON_ERROR_STOP=1)

# 动态枚举 public schema 下全部基表
CLEAR_LIST="$("${PSQL[@]}" -c "SELECT string_agg(quote_ident(tablename), ', ' ORDER BY tablename)
                                 FROM pg_tables
                                WHERE schemaname='public'
                                  AND tablename NOT IN ($KEEP_TABLES)")"
KEEP_LIST="$("${PSQL[@]}" -c "SELECT string_agg(quote_ident(tablename), ', ' ORDER BY tablename)
                                FROM pg_tables
                               WHERE schemaname='public'
                                 AND tablename IN ($KEEP_TABLES)")"

MODE="${1:-}"

if [ "$MODE" = "--list" ]; then
  echo "将保留: ${KEEP_LIST:-(无)}"
  echo "将清空: ${CLEAR_LIST:-(无)}"
  exit 0
fi

if [ -z "$CLEAR_LIST" ]; then
  echo "没有可清空的数据表，退出"
  exit 0
fi

TABLE_COUNT="$("${PSQL[@]}" -c "SELECT count(*) FROM pg_tables
                                 WHERE schemaname='public'
                                   AND tablename NOT IN ($KEEP_TABLES)")"
DB_NAME="$("${PSQL[@]}" -c "SELECT current_database()")"
# 展示用 DSN：隐藏密码
DB_URL_MASKED="$(echo "$DB_URL" | sed -E 's#//[^@/]+@#//***@#')"

echo "目标库: ${DB_NAME} @ ${DB_URL_MASKED}"
echo "将 TRUNCATE ${TABLE_COUNT} 张表（RESTART IDENTITY CASCADE）:"
echo "  ${CLEAR_LIST}"
echo "将保留: ${KEEP_LIST}"
echo

if [ "$MODE" != "--yes" ]; then
  read -r -p "⚠️  该操作不可恢复（先跑 ./scripts/backup.sh 可备份）。输入 yes 确认继续: " ANSWER
  [ "$ANSWER" = "yes" ] || { echo "已取消"; exit 1; }
fi

psql "$DB_URL" -X -v ON_ERROR_STOP=1 -c "TRUNCATE TABLE ${CLEAR_LIST} RESTART IDENTITY CASCADE"
echo "✅ 已清空 ${TABLE_COUNT} 张表；${KEEP_LIST} 原样保留"
