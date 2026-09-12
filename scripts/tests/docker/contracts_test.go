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

func TestEnvOverridesAndPassthrough(t *testing.T) {
	dir := deployment(t)
	put(t, filepath.Join(dir, ".env"), []byte("EXTRA_SYNTHETIC_SETTING=retained\nSUMPTER_ADMIN_HOST=127.0.0.1\nSUMPTER_ADMIN_PORT=18081\nRUST_LOG=warn\n"))
	svc := project(t, dir).Services["sumpter"]
	if *svc.Environment["EXTRA_SYNTHETIC_SETTING"] != "retained" {
		t.Fatal("additional env_file variable lost")
	}
	if *svc.Environment["RUST_LOG"] != "warn" {
		t.Fatal("RUST_LOG override lost")
	}
	// 内部监听契约不能被 .env 改写，否则容器健康检查与端口映射会错位。
	if *svc.Environment["SUMPTER_ADMIN_HOST"] != "0.0.0.0" || *svc.Environment["SUMPTER_ADMIN_PORT"] != "57879" {
		t.Fatal(".env must not override the in-container listener contract")
	}
}

func TestCustomPortsAndDataDirectory(t *testing.T) {
	dir := deployment(t)
	p := project(t, dir,
		"SUMPTER_PROXY_PORT=18080", "SUMPTER_ADMIN_PORT=18081", "SUMPTER_CONFIG_DIR=./state",
		"SUMPTER_PROXY_BIND_HOST=0.0.0.0", "SUMPTER_LOG_MAX_FILES=5")
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

	// 默认（无 .env）与带 .env 两种情况都必须引用官方 GHCR 镜像。
	if got := project(t, dir).Services["sumpter"].Image; got != "ghcr.io/domoxiaojun/sumpter:latest" {
		t.Fatalf("unexpected default image: %s", got)
	}
	if got := project(t, dir, "SUMPTER_IMAGE=ghcr.io/domoxiaojun/sumpter:0.4.6").Services["sumpter"].Image; got != "ghcr.io/domoxiaojun/sumpter:0.4.6" {
		t.Fatalf("image pinning lost: %s", got)
	}

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

// 环境变量示例必须覆盖模板引用的每一个变量，且不能把不生效的变量当可用参数提供。
func TestEnvInventoryStaysComplete(t *testing.T) {
	compose := string(read(t, filepath.Join(root(), "platforms/linux/compose.yaml")))
	sample := string(read(t, filepath.Join(root(), "platforms/linux/.env.example")))
	doc := string(read(t, filepath.Join(root(), "platforms/linux/DOCKER.md")))

	re := regexp.MustCompile(`\$\{([A-Z_][A-Z0-9_]*)`)
	seen := map[string]bool{}
	for _, match := range re.FindAllStringSubmatch(compose, -1) {
		name := match[1]
		if seen[name] {
			continue
		}
		seen[name] = true
		if !strings.Contains(sample, name) {
			t.Errorf(".env.example 缺少 %s：模板引用但示例未说明", name)
		}
		if !strings.Contains(doc, "`"+name+"`") {
			t.Errorf("DOCKER.md 参数表缺少 %s", name)
		}
	}
	if len(seen) < 10 {
		t.Fatalf("只解析到 %d 个变量，正则或模板结构可能已变", len(seen))
	}

	// 引擎显式 no_proxy()，这些变量不能作为可用参数出现在示例或模板插值里。
	for _, name := range []string{"HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"} {
		if strings.Contains(compose, "${"+name) {
			t.Errorf("compose.yaml 不应当把 %s 当作可用参数", name)
		}
		if regexp.MustCompile(`(?m)^` + name + `=`).MatchString(sample) {
			t.Errorf(".env.example 不应当把 %s 当作可用参数", name)
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
	for _, name := range []string{"platforms/linux/DOCKER.md", "platforms/linux/.env.example"} {
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
	for _, name := range []string{"platforms/linux/DOCKER.md", "platforms/linux/.env.example"} {
		for _, match := range imageTag.FindAllStringSubmatch(string(read(t, filepath.Join(root(), name))), -1) {
			if match[1] != version[2] {
				t.Errorf("%s 的镜像示例 %s 与 workspace 版本 %s 不一致", name, match[1], version[2])
			}
		}
	}
}

func TestDeploymentPackagingInputs(t *testing.T) {
	script := string(read(t, filepath.Join(root(), "platforms/linux/scripts/cross-build.sh")))
	list := "compose.yaml .env.example DOCKER.md"
	if strings.Count(script, "for docker_file in "+list+"; do") != 2 {
		t.Fatal("packaging must both preflight and copy every deployment input")
	}
	for _, file := range strings.Fields(list) {
		read(t, filepath.Join(root(), "platforms/linux", file))
	}
}
