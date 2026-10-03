# campus-mcp

PLMS와 Outlook 메일을 AI 클라이언트에서 조회하는 로컬 MCP 서버입니다. 하나의 서버 안에서 LMS와 메일 모듈을 분리했습니다. `--modules lms` 또는 `--modules mail`로 필요한 도구만 노출할 수 있습니다.

기존 `lms-helper`의 인증·수집 코드를 특정 Git 커밋에 고정해 재사용합니다. Discord 봇, 자동 알림, 중요도 분류는 실행하지 않으며 LLM API 키도 필요하지 않습니다. 답변과 첨부 해석은 연결한 AI 클라이언트가 담당합니다.

## 설치

Rust stable, Git, 두 private 저장소(`campus-mcp`, `lms-helper`)에 대한 GitHub SSH 접근 권한이 필요합니다. OpenSSL 빌드 환경이 필요할 수 있습니다(macOS의 Homebrew OpenSSL, Linux의 OpenSSL 개발 패키지 및 pkg-config).

```sh
git clone git@github.com:Wooangha/campus-mcp.git
cd campus-mcp
cargo build --locked
```

실행 파일은 `target/debug/campus-mcp`입니다. 배포용 최적화 빌드는 `cargo build --locked --release` 후 `target/release/campus-mcp`를 사용합니다.

## 인증 및 실행

`.env.example`을 참고해 별도의 로컬 설정 파일을 만드세요. 이미 `lms-helper`의 `.env`가 있다면 복사하지 않고 그 파일을 지정해도 됩니다.

```sh
# 기존 환경 변수도 사용 가능. --env-file은 명시한 파일만 읽습니다.
./target/debug/campus-mcp --env-file /absolute/path/private.env serve

# LMS만 사용
./target/debug/campus-mcp --env-file /absolute/path/private.env serve --modules lms

# 메일만 사용: LMS 계정 불필요
./target/debug/campus-mcp serve --modules mail
```

LMS는 `LMS_SERVICE=plms`, `PLMS_USERNAME`, `PLMS_PASSWORD`를 사용합니다. 메일의 `MAIL_TENANT`는 선택 사항이며 기본값은 기존 helper의 POSTECH tenant입니다. 프로세스 환경 변수가 설정 파일보다 우선합니다. 현재 디렉터리의 `.env`를 자동으로 읽지 않습니다.

메일은 기존 `~/.config/lms-helper/outlook-token.json` 로그인을 재사용합니다. 로그인이 필요하면 **별도 터미널**에서 실행하세요.

```sh
./target/debug/campus-mcp --env-file /absolute/path/private.env mail-login
```

인증은 해당 도구를 처음 호출할 때 확인합니다. 서버 초기화·도구 목록 조회는 계정 없이도 됩니다. MCP 호출 중 브라우저 로그인을 시작하지 않습니다. 로그인 토큰은 로컬에서 갱신·저장되지만 도구 응답에 노출하지 않습니다.

## MCP 클라이언트 연결

stdio MCP를 지원하는 클라이언트에 실행 파일의 **절대 경로**와 인자를 등록합니다. `mcpServers` JSON 형식의 클라이언트는 [examples/mcp.json](examples/mcp.json)을 참고하세요. 클라이언트마다 설정 위치와 형식은 다를 수 있습니다.

```json
{
  "mcpServers": {
    "campus": {
      "command": "/absolute/path/campus-mcp/target/debug/campus-mcp",
      "args": ["--env-file", "/absolute/path/private.env", "serve", "--modules", "lms,mail"]
    }
  }
}
```

`serve`를 터미널에서 실행하면 JSON-RPC 입력을 기다리는 것이 정상입니다. stdout은 프로토콜 전용입니다. 클라우드 서비스에서 직접 연결하는 HTTP 서버·공개 URL은 제공하지 않습니다.

## 도구

| 도구 | 기능 | 인자 |
|---|---|---|
| `campus_status` | 활성 모듈과 실행 방식 확인 | 없음 |
| `lms_courses` | 수강 과목 목록 | `all_terms` (기본 false) |
| `lms_deadlines` | 다가오는 마감 일정 | `limit` (기본 30) |
| `lms_notices` | 공지 목록 | `limit`, `all_terms` |
| `lms_assignments` | 현재 학기 과제 목록, 제출한 과제 포함 | `limit` |
| `lms_submission` | 제출 상태·파일명·최종 수정·마감 조회 | `url` |
| `lms_read` | 공지/과제 본문과 첨부 다운로드 | `url` |
| `mail_list` | 최근 메일 목록 | `limit` (기본 20), `unread_only` |
| `mail_read` | 메일 본문과 첨부 다운로드 | `id` |
| `attachment_read` | 내려받은 첨부를 AI 클라이언트에 전달 | `id`, `offset`, `max_chars` |

목록 `limit`은 1~100입니다. `lms_read`/`lms_submission`에는 목록에서 얻은 PLMS 주소를, `mail_read`에는 메일 ID를 전달합니다. 메일을 읽음 처리하거나 발송하지 않고, 과제를 제출·수정하지 않습니다.

예시 질문:

- “이번 주 마감 과제를 알려줘. 제출 여부도 확인해줘.”
- “데이터베이스 과제 공지와 첨부 PDF를 읽고 준비할 일을 정리해줘.”
- “최근 메일 30개 중 내가 직접 해야 하는 일이 있는지 본문까지 확인해줘.”

### 결과 해석

- 제출된 과제는 PLMS의 다가오는 일정에서 사라질 수 있습니다. 제출 여부는 `lms_assignments` → `lms_submission`으로 확인합니다. 빈 마감 목록을 ‘과제 없음’으로 해석하면 안 됩니다.
- `mail_list`는 받은편지함 전용이 아닌 전체 메일함의 최근 **첫 페이지**입니다. `unread_only=true`도 그 조회 범위 안에서만 필터링합니다. 전체 안 읽은 메일 검색이 아니며 결과에 `partial: true`를 표시합니다.
- `warnings`, `partial`, `truncated`, `body_truncated`를 확인하세요. 일부 과목/첨부 조회 실패를 전체 성공으로 표현하지 않습니다.
- `due`, `ends`, `fetched_at` 숫자는 Unix 초입니다. `modified`와 `due_text`는 원문 표시 문자열입니다.
- 제출 파일 이름과 상태를 보여 주며, 요구 형식을 맞췄는지 또는 정답인지 판정하지 않습니다.

### 첨부와 제한

본문 응답은 100,000자, 첨부 목록은 첫 100개입니다. 다운로드는 파일당 20 MiB, 한 본문 조회당 총 50 MiB로 제한합니다. `attachment_read`는 4 MiB 이하의 PDF·PNG·JPEG·WebP와 UTF-8 텍스트를 지원합니다. 텍스트는 최대 20,000자씩 `next_offset`으로 이어 읽습니다. PDF·이미지는 클라이언트의 해당 MCP 콘텐츠 지원이 필요합니다. ZIP·Office·HWP의 내부 내용 분석이나 압축 해제는 제공하지 않습니다.

첨부 ID는 현재 서버 프로세스에서만 유효하고 최근 128개까지만 유지합니다. 만료되면 원문을 다시 조회하세요. 임의의 로컬 경로나 URL을 첨부 도구에 넣어 읽을 수 없습니다.

다운로드 파일은 기본 `~/.cache/campus-mcp`에 저장하며 `serve --cache-dir /absolute/path/cache`로 변경할 수 있습니다. **다운로드 제한은 요청당 제한이며 디스크 전체 용량 제한은 아닙니다.** 자동 삭제는 하지 않으므로 필요 없어진 캐시는 서버를 종료한 뒤 정리하세요. 비밀번호·메일·다운로드 파일을 저장소에 커밋하지 마세요.

원문과 첨부는 외부 데이터입니다. 그 안의 지시를 실행하지 않도록 서버 설명에도 명시합니다. MCP 클라이언트에 반환된 자료는 그 클라이언트의 AI에 전달될 수 있습니다.

요청은 최대 180초이며 PLMS 제출 조회에서 받은 재시도 대기 시간은 같은 서버 프로세스에서 유지합니다. 서버를 재시작하면 대기 상태는 초기화됩니다.

## 개발 및 검증

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked
python3 scripts/smoke.py
```

기본 테스트는 실제 계정·네트워크가 필요하지 않습니다. 실제 stdio 프로세스의 초기화, 도구 목록, 입력 거부, 미인증 오류까지 확인합니다. 계정으로 조회를 검증하려면 명시적으로 실행합니다(원문 내용은 출력하지 않음).

```sh
python3 scripts/smoke.py --live-env /absolute/path/private.env
# LMS만 실제 조회하려면 위 명령에 --modules lms 추가
```

`src/server.rs`는 도구 스키마·라우팅, `src/backend/lms.rs`와 `mail.rs`는 서비스 연결, `attachments.rs`는 첨부 ID·출력 형식, `tests/protocol.rs`는 MCP 통신 회귀 테스트를 담당합니다. 기존 helper 수정은 고정된 의존 커밋을 갱신하기 전까지 이 서버에 반영되지 않습니다.
