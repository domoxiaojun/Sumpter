package docker_test

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"strings"
	"testing"

	"github.com/compose-spec/compose-go/v2/cli"
	"github.com/compose-spec/compose-go/v2/types"
	"github.com/moby/patternmatcher"
	"github.com/moby/patternmatcher/ignorefile"
)

func root() string {
	_, file, _, _ := runtime.Caller(0)
	return filepath.Clean(filepath.Join(filepath.Dir(file), "../../.."))
}

func read(t *testing.T, path string) []byte {
	t.Helper()
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return b
}

func put(t *testing.T, path string, content []byte) {
	t.Helper()
	if err := os.WriteFile(path, content, 0600); err != nil {
		t.Fatal(err)
	}
}

// 用 Docker 自己的解析器/匹配器验证构建上下文隔离，而不是近似实现 glob。
func TestBuildContextIsolation(t *testing.T) {
	cases := []struct {
		name, file      string
		allowed, denied []string
	}{
		{"source", ".dockerignore", []string{
			"Cargo.toml", "Cargo.lock", "crates/sumpter-core/src/lib.rs",
			"adapters/linux/sumpter-linux-adapter/src/engine.rs", "apps/linux/sumpterd/src/main.rs",
			"platforms/linux/scripts/setup-client-attribution.sh", "platforms/linux/scripts/client-attribution.mjs",
			"platforms/linux/web/index.html", "platforms/linux/web/assets/synthetic.js", "platforms/linux/config.example.json",
		}, []string{
			"target/debug/sumpterd", "platforms/linux/target/debug/sumpterd", ".git/config", ".env",
			"platforms/linux/.env", "platforms/linux/config/admin-password", "platforms/linux/config/config.json",
			"platforms/linux/config/runtime.sqlite3", "platforms/linux/config/diagnostic_capture.json",
			"platforms/linux/webui/node_modules/react/index.js", "platforms/linux/dist/sumpter-linux-x86_64/sumpterd",
			"platforms/macos/app/.build/debug.yaml", "platforms/macos/app/dist/Sumpter.dmg",
			"crates/sumpter-core/.env", "adapters/linux/sumpter-linux-adapter/target/debug/synthetic",
		}},
		{"runtime", "platforms/linux/.dockerignore", []string{
			"Dockerfile.runtime", "docker-bin/sumpterd-amd64", "docker-bin/sumpterd-arm64",
			"web/index.html", "web/assets/synthetic.js", "config.example.json",
		}, []string{
			".env", "config/admin-password", "config/config.json", "config/runtime.sqlite3",
			"config/diagnostic_capture.json", "webui/node_modules/react/index.js", "dist/sumpter-linux-x86_64/sumpterd",
		}},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			patterns, err := ignorefile.ReadAll(strings.NewReader(string(read(t, filepath.Join(root(), c.file)))))
			if err != nil {
				t.Fatal(err)
			}
			pm, err := patternmatcher.New(patterns)
			if err != nil {
				t.Fatal(err)
			}
			for _, group := range []struct {
				wantIgnored bool
				paths       []string
			}{{false, c.allowed}, {true, c.denied}} {
				for _, path := range group.paths {
					ignored, err := pm.MatchesOrParentMatches(path)
					if err != nil {
						t.Fatal(err)
					}
					var info patternmatcher.MatchInfo
					var walked bool
					parts := strings.Split(path, "/")
					for i := range parts {
						walked, info, err = pm.MatchesUsingParentResults(strings.Join(parts[:i+1], "/"), info)
						if err != nil {
							t.Fatal(err)
						}
					}
					if ignored != group.wantIgnored || walked != group.wantIgnored {
						t.Errorf("%s: ignored=%v walked=%v want=%v", path, ignored, walked, group.wantIgnored)
					}
				}
			}
		})
	}
}

func deployment(t *testing.T) string {
	t.Helper()
	dir := filepath.Join(t.TempDir(), "sumpter")
	if err := os.MkdirAll(filepath.Join(dir, "config"), 0700); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"compose.yaml", "compose.bridge.example.yaml", "compose.build.example.yaml"} {
		put(t, filepath.Join(dir, name), read(t, filepath.Join(root(), "platforms/linux", name)))
	}
	return dir
}

func load(t *testing.T, dir string, files, env []string) (*types.Project, error) {
	t.Helper()
	paths := make([]string, len(files))
	for i, name := range files {
		paths[i] = filepath.Join(dir, name)
	}
	// 复刻 docker compose CLI：先读工作目录里的 .env 再插值，缺失的 .env 不算错误。
	opts, err := cli.NewProjectOptions(paths,
		cli.WithName("sumpter-test"),
		cli.WithWorkingDirectory(dir),
		cli.WithEnvFiles(),
		cli.WithDotEnv,
		cli.WithEnv(env))
	if err != nil {
		return nil, err
	}
	return opts.LoadProject(context.Background())
}

func project(t *testing.T, dir string, extra ...string) *types.Project {
	t.Helper()
	p, err := load(t, dir, []string{"compose.yaml"}, extra)
	if err != nil {
		t.Fatal(err)
	}
	return p
}

func TestStandaloneDeployment(t *testing.T) {
	dir := deployment(t)
	// 不带 .env、不带任何变量也必须能直接启动，并使用镜像默认用户。
	p := project(t, dir)
	for _, name := range []string{"init", "sumpter"} {
		svc := p.Services[name]
		if svc.User != "" {
			t.Fatalf("%s must not pin a container user: %q", name, svc.User)
		}
		if !svc.ReadOnly || len(svc.CapDrop) != 1 || svc.CapDrop[0] != "ALL" {
			t.Fatalf("%s security mismatch", name)
		}
		if len(svc.Volumes) != 1 || svc.Volumes[0].Source != filepath.Join(dir, "config") || svc.Volumes[0].Target != "/config" {
			t.Fatalf("%s mount mismatch: %+v", name, svc.Volumes)
		}
	}
	if p.Services["init"].Image != p.Services["sumpter"].Image {
		t.Fatal("init must use the daemon image")
	}
	// 部署模板不能自带 build，否则会变成在主机构建镜像。
	for _, name := range []string{"init", "sumpter"} {
		if p.Services[name].Build != nil {
			t.Fatalf("%s must not define build in the deployment template", name)
		}
	}
	svc := p.Services["sumpter"]
	if svc.NetworkMode == "host" {
		t.Fatal("default must be bridge")
	}
	if svc.DependsOn["init"].Condition != "service_completed_successfully" {
		t.Fatal("init success must gate startup")
	}
	if *svc.Environment["SUMPTER_ADMIN_HOST"] != "0.0.0.0" || *svc.Environment["SUMPTER_ADMIN_PORT"] != "57879" {
		t.Fatal("daemon must listen on fixed in-container addresses")
	}
	if *svc.Environment["SUMPTER_ADMIN_PASSWORD_FILE"] != "/config/admin-password" {
		t.Fatal("password file contract changed")
	}
	for _, port := range svc.Ports {
		if port.HostIP != "127.0.0.1" {
			t.Fatalf("public by default: %+v", port)
		}
	}
	if len(svc.Ports) != 2 {
		t.Fatalf("expected proxy+admin ports, got %+v", svc.Ports)
	}
}

// 探针住在镜像里：compose 不再声明 healthcheck，容器健康状态由 Dockerfile 的
// HEALTHCHECK 提供。这里守住这个性质的两个半边，避免“模板删了、镜像也没有”的真空。
func TestHealthcheckLivesInImageMetadata(t *testing.T) {
	for _, name := range []string{"platforms/linux/Dockerfile", "platforms/linux/Dockerfile.runtime"} {
		text := string(read(t, filepath.Join(root(), name)))
		if !strings.Contains(text, "HEALTHCHECK") {
			t.Errorf("%s 必须自带 HEALTHCHECK，否则容器没有任何存活探针", name)
		}
		// 探针只能打容器内 Admin 端口，不能被宿主机端口映射影响。
		if !strings.Contains(text, "${SUMPTER_ADMIN_PORT:-57879}/healthz") {
			t.Errorf("%s 的探针必须按 SUMPTER_ADMIN_PORT 取容器内 Admin 端口", name)
		}
	}
	if svc := project(t, deployment(t)).Services["sumpter"]; svc.HealthCheck != nil {
		t.Error("compose.yaml 不应重复声明 healthcheck，探针以镜像元数据为准")
	}
}

func TestLegacyEnvDoesNotChangeDeployment(t *testing.T) {
	dir := deployment(t)
	put(t, filepath.Join(dir, ".env"), []byte("EXTRA_SYNTHETIC_SETTING=retained\nSUMPTER_ADMIN_HOST=127.0.0.1\nSUMPTER_ADMIN_PORT=18081\nRUST_LOG=warn\nSUMPTER_IMAGE=wrong:legacy\nSUMPTER_CONFIG_DIR=./old\nSUMPTER_PROXY_BIND_HOST=0.0.0.0\n"))
	p := project(t, dir)
	svc := p.Services["sumpter"]
	if _, present := svc.Environment["EXTRA_SYNTHETIC_SETTING"]; present || len(svc.EnvFiles) != 0 {
		t.Fatal("deployment must not import .env into the daemon")
	}
	if *svc.Environment["RUST_LOG"] != "info" || *svc.Environment["SUMPTER_ADMIN_HOST"] != "0.0.0.0" || *svc.Environment["SUMPTER_ADMIN_PORT"] != "57879" {
		t.Fatal("legacy .env changed fixed daemon defaults")
	}
	for _, name := range []string{"init", "sumpter"} {
		if p.Services[name].Image != "ghcr.io/domoxiaojun/sumpter:latest" || p.Services[name].Volumes[0].Source != filepath.Join(dir, "config") {
			t.Fatal("legacy .env changed image or data directory")
		}
	}
	for _, port := range svc.Ports {
		if port.HostIP != "127.0.0.1" || port.Published != fmt.Sprint(port.Target) {
			t.Fatalf("legacy .env changed published port: %+v", port)
		}
	}
}

func TestCustomPortsAndDataDirectory(t *testing.T) {
	dir := deployment(t)
	file := filepath.Join(dir, "compose.yaml")
	edited := strings.NewReplacer(
		"127.0.0.1:57878:57878", "0.0.0.0:18080:57878",
		"127.0.0.1:57879:57879", "127.0.0.1:18081:57879",
		"./config:/config", "./state:/config",
		`max-file: "3"`, `max-file: "5"`,
	).Replace(string(read(t, file)))
	put(t, file, []byte(edited))
	p := project(t, dir)
	svc := p.Services["sumpter"]
	expected := map[uint32]string{57878: "18080", 57879: "18081"}
	for _, port := range svc.Ports {
		if expected[port.Target] != port.Published {
			t.Fatalf("port mismatch: %+v", port)
		}
		delete(expected, port.Target)
	}
	if len(expected) != 0 {
		t.Fatal("published port missing")
	}
	// 探针已移入镜像元数据；模板不得重复声明，否则会与镜像漂移。
	if svc.HealthCheck != nil {
		t.Fatal("compose.yaml must not redeclare a healthcheck")
	}
	for _, name := range []string{"init", "sumpter"} {
		if p.Services[name].Volumes[0].Source != filepath.Join(dir, "state") {
			t.Fatalf("%s data dir not overridable", name)
		}
	}
	if svc.Logging.Options["max-file"] != "5" {
		t.Fatal("log options not overridable")
	}
	if svc.Ports[0].HostIP != "0.0.0.0" {
		t.Fatal("bind host not overridable")
	}
}

func TestPullsGhcrImageAndBuildOverlayIsIsolated(t *testing.T) {
	dir := deployment(t)

	// 镜像默认值与直接编辑后的版本同时应用于 init 和 daemon。
	if got := project(t, dir).Services["sumpter"].Image; got != "ghcr.io/domoxiaojun/sumpter:latest" {
		t.Fatalf("unexpected default image: %s", got)
	}
	original := read(t, filepath.Join(dir, "compose.yaml"))
	put(t, filepath.Join(dir, "compose.yaml"), []byte(strings.ReplaceAll(string(original), "sumpter:latest", "sumpter:0.4.6")))
	for name, svc := range project(t, dir).Services {
		if svc.Image != "ghcr.io/domoxiaojun/sumpter:0.4.6" {
			t.Fatalf("%s image pinning lost: %s", name, svc.Image)
		}
	}
	put(t, filepath.Join(dir, "compose.yaml"), original)

	// 源码构建 override 使用独立本地 tag，绝不继承 SUMPTER_IMAGE，
	// 否则会把本机构建结果打成正式 GHCR 名称。
	p, err := load(t, filepath.Join(root(), "platforms/linux"),
		[]string{"compose.yaml", "compose.build.example.yaml"},
		[]string{"SUMPTER_IMAGE=ghcr.io/domoxiaojun/sumpter:latest"})
	if err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"init", "sumpter"} {
		svc := p.Services[name]
		if svc.Build == nil || svc.Build.Context != root() || svc.Build.Dockerfile != "platforms/linux/Dockerfile" {
			t.Fatalf("%s source build mismatch", name)
		}
		if svc.Image != "sumpter:local" {
			t.Fatalf("%s build overlay must not reuse the GHCR image name: %s", name, svc.Image)
		}
		if svc.User != "" {
			t.Fatalf("%s overlay must not pin a user", name)
		}
		// override 只应换镜像与 build，不得丢失挂载与只读加固。
		if !svc.ReadOnly || len(svc.CapDrop) != 1 || svc.CapDrop[0] != "ALL" {
			t.Fatalf("%s overlay lost the security hardening", name)
		}
		if len(svc.Volumes) != 1 || svc.Volumes[0].Target != "/config" {
			t.Fatalf("%s overlay lost the /config mount: %+v", name, svc.Volumes)
		}
		// init 是一次性任务：只能出现 daemon 的端口；探针来自镜像，模板不声明。
		if name == "sumpter" {
			if len(svc.Ports) != 2 || svc.HealthCheck != nil {
				t.Fatalf("%s overlay ports/healthcheck mismatch: ports=%v hc=%v", name, svc.Ports, svc.HealthCheck)
			}
		} else if len(svc.Ports) != 0 || svc.HealthCheck != nil || svc.NetworkMode != "none" {
			t.Fatalf("init must stay network-less and port-less, got ports=%v hc=%v net=%s", svc.Ports, svc.HealthCheck, svc.NetworkMode)
		}
	}
	if got := project(t, dir, "SUMPTER_LOCAL_IMAGE=sumpter:dev").Services["sumpter"].Image; got != "ghcr.io/domoxiaojun/sumpter:latest" {
		t.Fatalf("local image variable must not affect the deploy template: %s", got)
	}
	if string(read(t, filepath.Join(dir, "compose.bridge.example.yaml"))) != string(read(t, filepath.Join(dir, "compose.yaml"))) {
		t.Fatal("bridge compatibility entry must stay identical to canonical compose")
	}
}

func initScript(t *testing.T) string {
	t.Helper()
	svc := project(t, deployment(t)).Services["init"]
	if strings.Join(svc.Entrypoint, " ") != "/bin/sh -ec" || len(svc.Command) != 1 || svc.WorkingDir != "/config" {
		t.Fatal("unexpected init execution contract")
	}
	return svc.Command[0]
}

func runInit(t *testing.T, script, dir string) error {
	t.Helper()
	command := exec.Command("/bin/sh", "-ec", script)
	command.Dir = dir
	output, err := command.CombinedOutput()
	if err != nil {
		return fmt.Errorf("init: %w: %s", err, output)
	}
	return nil
}

func TestInitCreatesPrivateFilesAndPreservesState(t *testing.T) {
	dir := t.TempDir()
	script := initScript(t)
	if err := runInit(t, script, dir); err != nil {
		t.Fatal(err)
	}
	config := read(t, filepath.Join(dir, "config.json"))
	password := read(t, filepath.Join(dir, "admin-password"))
	if len(password) != 65 {
		t.Fatalf("expected 32 random bytes as hex plus newline, got %d bytes", len(password))
	}
	var value struct {
		Schema    int   `json:"schemaVersion"`
		Endpoints []any `json:"endpoints"`
		Listener  struct {
			Host      string `json:"host"`
			Port      int    `json:"port"`
			AuthToken string `json:"authToken"`
		} `json:"listener"`
	}
	if err := json.Unmarshal(config, &value); err != nil {
		t.Fatal(err)
	}
	var example struct {
		Schema int `json:"schemaVersion"`
	}
	if err := json.Unmarshal(read(t, filepath.Join(root(), "config.example.json")), &example); err != nil {
		t.Fatal(err)
	}
	// bridge 部署必须在容器内监听非回环地址，否则端口映射无法访问代理。
	if value.Schema != example.Schema || len(value.Endpoints) != 0 || value.Listener.Host != "0.0.0.0" || value.Listener.Port != 57878 || value.Listener.AuthToken != "" {
		t.Fatalf("unsafe or invalid bootstrap config: %+v", value)
	}
	for path, mode := range map[string]os.FileMode{
		dir: 0700, filepath.Join(dir, "config.json"): 0600, filepath.Join(dir, "admin-password"): 0600,
	} {
		stat, err := os.Stat(path)
		if err != nil || stat.Mode().Perm() != mode {
			t.Fatalf("bad permissions on %s: %v", path, err)
		}
	}
	put(t, filepath.Join(dir, "runtime.sqlite3"), []byte("synthetic-existing-database"))
	put(t, filepath.Join(dir, "resource_bindings.json"), []byte("synthetic-existing-bindings"))
	if err := runInit(t, script, dir); err != nil {
		t.Fatal(err)
	}
	for name, expected := range map[string]string{
		"config.json": string(config), "admin-password": string(password),
		"runtime.sqlite3": "synthetic-existing-database", "resource_bindings.json": "synthetic-existing-bindings",
	} {
		if string(read(t, filepath.Join(dir, name))) != expected {
			t.Fatalf("init overwrote %s", name)
		}
	}
	other := t.TempDir()
	if err := runInit(t, script, other); err != nil {
		t.Fatal(err)
	}
	if string(read(t, filepath.Join(other, "admin-password"))) == string(password) {
		t.Fatal("init reused a password across directories")
	}
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatal(err)
	}
	for _, entry := range entries {
		if strings.HasPrefix(entry.Name(), ".sumpter-init.") {
			t.Fatal("init leaked its staging directory")
		}
	}
}

// 首次启动要直接把初始密码写进日志：容器里没有安装器那样的交互终端，让用户去宿主机
// cat 文件是最大的首次使用摩擦点。约束是“只打印一次”：重启不得重复刷已有密码。
func TestInitPrintsPasswordOnceOnFirstRun(t *testing.T) {
	dir := t.TempDir()
	script := initScript(t)
	run := func() string {
		t.Helper()
		command := exec.Command("/bin/sh", "-ec", script)
		command.Dir = dir
		output, err := command.CombinedOutput()
		if err != nil {
			t.Fatalf("init: %v: %s", err, output)
		}
		return string(output)
	}
	first := run()
	password := strings.TrimSpace(string(read(t, filepath.Join(dir, "admin-password"))))
	if len(password) != 64 {
		t.Fatalf("expected 64 hex chars, got %d", len(password))
	}
	if !strings.Contains(first, password) {
		t.Fatalf("首次启动必须在日志里给出初始密码: %s", first)
	}
	if !strings.Contains(first, "admin-password") || !strings.Contains(first, "WebUI") {
		t.Fatalf("日志必须给出凭据路径与改密提醒: %s", first)
	}
	if second := run(); strings.Contains(second, password) {
		t.Fatalf("重启不得重复打印已有密码: %s", second)
	}
}

func TestInitRejectsLinksDirectoriesAndLegacyOnly(t *testing.T) {
	script := initScript(t)
	for _, kind := range []string{"symlink", "directory", "legacy"} {
		t.Run(kind, func(t *testing.T) {
			dir := t.TempDir()
			var err error
			switch kind {
			case "symlink":
				err = os.Symlink("missing-target", filepath.Join(dir, "admin-password"))
			case "directory":
				err = os.Mkdir(filepath.Join(dir, "admin-password"), 0700)
			case "legacy":
				err = os.WriteFile(filepath.Join(dir, "keys.json"), []byte("legacy"), 0600)
			}
			if err != nil {
				t.Fatal(err)
			}
			if err := runInit(t, script, dir); err == nil {
				t.Fatal("unsafe init input accepted")
			}
			if _, err := os.Stat(filepath.Join(dir, "config.json")); !os.IsNotExist(err) {
				t.Fatal("config.json created even though init refused to run")
			}
		})
	}
}

func TestInitScriptShellcheck(t *testing.T) {
	binary, err := exec.LookPath("shellcheck")
	if err != nil {
		t.Skip("shellcheck unavailable")
	}
	cmd := exec.Command(binary, "--shell=sh", "-")
	cmd.Stdin = strings.NewReader(initScript(t))
	if output, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("shellcheck: %v\n%s", err, output)
	}
}

// 独立部署模板不依赖 .env；默认值必须直接写在 Compose 中。
func TestDeploymentHasNoEnvDependency(t *testing.T) {
	compose := string(read(t, filepath.Join(root(), "platforms/linux/compose.yaml")))
	doc := string(read(t, filepath.Join(root(), "platforms/linux/DOCKER.md")))

	re := regexp.MustCompile(`\$\{([A-Z_][A-Z0-9_]*)`)
	seen := map[string]bool{}
	for _, match := range re.FindAllStringSubmatch(compose, -1) {
		seen[match[1]] = true
	}
	if strings.Contains(compose, "env_file:") || len(seen) != 0 {
		t.Fatalf("compose.yaml 不应依赖 .env 插值，发现 %v", seen)
	}

	// 引擎显式 no_proxy()，这些变量不能作为可用参数出现在示例或模板插值里。
	for _, name := range []string{"HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"} {
		if strings.Contains(compose, "${"+name) {
			t.Errorf("compose.yaml 不应当把 %s 当作可用参数", name)
		}
		if !strings.Contains(doc, name) {
			t.Errorf("DOCKER.md 应说明 %s 为什么不生效", name)
		}
	}
}

// 部署文档的轻量结构守卫：代码围栏成对，列表不被拼进上一行句子。
func TestDeploymentDocsStructure(t *testing.T) {
	docs := []string{
		"platforms/linux/DOCKER.md", "platforms/linux/README.md", "platforms/linux/config/README.md",
		"docs/project-structure.md", "docs/development.md", "docs/releasing.md",
		"scripts/README.md", "scripts/tests/docker/README.md",
	}
	mergedBullet := regexp.MustCompile(`[。；]- `)
	for _, name := range docs {
		t.Run(name, func(t *testing.T) {
			lines := strings.Split(string(read(t, filepath.Join(root(), name))), "\n")
			fences := 0
			for i, line := range lines {
				if strings.HasPrefix(strings.TrimSpace(line), "```") {
					fences++
				}
				if mergedBullet.MatchString(line) {
					t.Errorf("%s:%d 列表项被拼进上一行，检查换行", name, i+1)
				}
			}
			if fences%2 != 0 {
				t.Errorf("%s 代码围栏未成对（%d 个）", name, fences)
			}
		})
	}
}

// 镜像必须能支撑文档承诺的时区行为，且版本示例不能漂移。
func TestRuntimeImageAndVersionExamples(t *testing.T) {
	for _, name := range []string{"platforms/linux/Dockerfile", "platforms/linux/Dockerfile.runtime"} {
		if !strings.Contains(string(read(t, filepath.Join(root(), name))), "tzdata") {
			t.Errorf("%s 必须安装 tzdata，否则 TZ 不生效", name)
		}
	}

	// 文档仍声称“未安装 tzdata”就说明镜像与文档已经不一致。
	for _, name := range []string{"platforms/linux/DOCKER.md"} {
		if strings.Contains(string(read(t, filepath.Join(root(), name))), "未安装 tzdata") {
			t.Errorf("%s 仍写着未安装 tzdata", name)
		}
	}

	// ghcr.io/domoxiaojun/sumpter:<x.y.z> 示例必须与 workspace 版本一致，避免发版后文档漂移。
	cargo := string(read(t, filepath.Join(root(), "Cargo.toml")))
	version := regexp.MustCompile(`(?s)\[workspace\.package\](.*?)version = "([^"]+)"`).FindStringSubmatch(cargo)
	if version == nil {
		t.Fatal("无法从 Cargo.toml 读取 workspace 版本")
	}
	imageTag := regexp.MustCompile(`domoxiaojun/sumpter:(\d+\.\d+\.\d+)`)
	for _, name := range []string{"platforms/linux/DOCKER.md"} {
		for _, match := range imageTag.FindAllStringSubmatch(string(read(t, filepath.Join(root(), name))), -1) {
			if match[1] != version[2] {
				t.Errorf("%s 的镜像示例 %s 与 workspace 版本 %s 不一致", name, match[1], version[2])
			}
		}
	}
}

func TestDeploymentPackagingInputs(t *testing.T) {
	script := string(read(t, filepath.Join(root(), "platforms/linux/scripts/cross-build.sh")))
	list := "compose.yaml DOCKER.md"
	if strings.Count(script, "for docker_file in "+list+"; do") != 2 {
		t.Fatal("packaging must both preflight and copy every deployment input")
	}
	for _, file := range strings.Fields(list) {
		read(t, filepath.Join(root(), "platforms/linux", file))
	}
}
