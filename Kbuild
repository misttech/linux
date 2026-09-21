# SPDX-License-Identifier: GPL-2.0
#
# Kbuild for top-level directory of the kernel

# Prepare global headers and check sanity before descending into sub-directories
# ---------------------------------------------------------------------------

# Generate bounds.h

bounds-file := include/generated/bounds.h

targets := kernel/bounds.s

$(bounds-file): kernel/bounds.s FORCE
	$(call filechk,offsets,__LINUX_BOUNDS_H__)

# Generate timeconst.h

timeconst-file := include/generated/timeconst.h

filechk_gentimeconst = echo $(CONFIG_HZ) | bc -q $<

$(timeconst-file): kernel/time/timeconst.bc FORCE
	$(call filechk,gentimeconst)

# Generate asm-offsets.h

offsets-file := include/generated/asm-offsets.h

targets += arch/$(SRCARCH)/kernel/asm-offsets.s

arch/$(SRCARCH)/kernel/asm-offsets.s: $(timeconst-file) $(bounds-file)

$(offsets-file): arch/$(SRCARCH)/kernel/asm-offsets.s FORCE
	$(call filechk,offsets,__ASM_OFFSETS_H__)

# Generate per-config C layout facts for the in-place Rust ports. This is a
# second pass because lockref.h itself includes bounds.h.
port-layout-header := include/generated/port-layout.h
port-layout-rs := include/generated/port-layout.rs
port-layout-cfg := include/generated/port-layout.cfg

targets += kernel/port-layout.s

kernel/port-layout.s: $(bounds-file)

$(port-layout-header): kernel/port-layout.s FORCE
	$(call filechk,offsets,__PORT_LAYOUT_H__)

define filechk_port_layout_rs
	echo "// SPDX-License-Identifier: GPL-2.0"; \
	echo "// Generated from the active C kernel configuration."; \
	awk '/^#define PORT_(LOCKREF|RATELIMIT|LWQ)_/ { \
		type = ($$2 == "PORT_LOCKREF_DEAD_VAL" ? "i32" : "usize"); \
		val = $$3; \
		if (type == "usize" && val ~ /^-/) val = "(" val "_isize) as usize"; \
		print "pub(crate) const " $$2 ": " type " = " val ";"; \
		if ($$2 == "PORT_LOCKREF_ALIGN") align = $$3; \
	} END { \
		print "#[repr(C, align(" align "))]"; \
		print "pub(crate) struct LockrefAlign;"; \
	}' $<
endef

$(port-layout-rs): $(port-layout-header) FORCE
	$(call filechk,port_layout_rs)

define filechk_port_layout_cfg
	awk '/^#define PORT_LOCKREF_FAST 1 / { print "--cfg=PORT_LOCKREF_FAST" }' $<
endef

$(port-layout-cfg): $(port-layout-header) FORCE
	$(call filechk,port_layout_cfg)

# Generate rq-offsets.h

rq-offsets-file := include/generated/rq-offsets.h

targets += kernel/sched/rq-offsets.s

kernel/sched/rq-offsets.s: $(offsets-file)

$(rq-offsets-file): kernel/sched/rq-offsets.s FORCE
	$(call filechk,offsets,__RQ_OFFSETS_H__)

# Check for missing system calls

missing-syscalls-file := .tmp_missing-syscalls$(missing_syscalls_instance)

targets += $(missing-syscalls-file)

quiet_cmd_syscalls = CALL    $< $(addprefix for ,$(missing_syscalls_instance))
      cmd_syscalls = DEPFILE=$(depfile) $(CONFIG_SHELL) $< $(CC) $(c_flags) $(missing_syscalls_flags); touch $@

$(missing-syscalls-file): scripts/checksyscalls.sh $(rq-offsets-file) FORCE
	$(call if_changed_dep,syscalls)

PHONY += missing-syscalls
missing-syscalls: $(missing-syscalls-file)

# Check the manual modification of atomic headers

quiet_cmd_check_sha1 = CHKSHA1 $<
      cmd_check_sha1 = \
	if ! command -v sha1sum >/dev/null; then \
		echo "warning: cannot check the header due to sha1sum missing"; \
		exit 0; \
	fi; \
	if [ "$$(sed -n '$$s:// ::p' $<)" != \
	     "$$(sed '$$d' $< | sha1sum | sed 's/ .*//')" ]; then \
		echo "error: $< has been modified." >&2; \
		exit 1; \
	fi; \
	touch $@

atomic-checks += $(addprefix $(obj)/.checked-, \
	  atomic-arch-fallback.h \
	  atomic-instrumented.h \
	  atomic-long.h)

targets += $(atomic-checks)
$(atomic-checks): $(obj)/.checked-%: include/linux/atomic/%  FORCE
	$(call if_changed,check_sha1)

# A phony target that depends on all the preparation targets

PHONY += prepare
prepare: $(offsets-file) missing-syscalls $(atomic-checks) $(if $(CONFIG_RUST_KERNEL),$(port-layout-rs) $(port-layout-cfg))
	@:

# Ordinary directory descending
# ---------------------------------------------------------------------------

obj-y			+= init/
obj-y			+= usr/
obj-y			+= arch/$(SRCARCH)/
obj-y			+= $(ARCH_CORE)
obj-y			+= kernel/
obj-y			+= certs/
obj-y			+= mm/
obj-y			+= fs/
obj-y			+= ipc/
obj-y			+= security/
obj-y			+= crypto/
obj-$(CONFIG_BLOCK)	+= block/
obj-$(CONFIG_IO_URING)	+= io_uring/
obj-$(CONFIG_RUST)	+= rust/
obj-y			+= $(ARCH_LIB)
obj-y			+= drivers/
obj-y			+= sound/
obj-$(CONFIG_SAMPLES)	+= samples/
obj-$(CONFIG_NET)	+= net/
obj-y			+= virt/
obj-y			+= $(ARCH_DRIVERS)
obj-$(CONFIG_DRM_HEADER_TEST)	+= include/
