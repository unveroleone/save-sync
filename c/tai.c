#include "sqlite3.h"
#include "vitashell_user.h"

#include <psp2/appmgr.h>
#include <psp2/io/dirent.h>
#include <psp2/kernel/modulemgr.h>
#include <psp2/registrymgr.h>
#include <psp2/sqlite.h>
#include <psp2/sysmodule.h>
#include <psp2/vshbridge.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <taihen.h>

#define UNUSED(x) __attribute__((unused)) x
#define MAX_MOUNT_POINT_LENGTH 16
#define PSV_APP_DB "ur0:/shell/db/app.db"
#define PLUGIN_KERNEL_PATH "ux0:app/SAVSYNC01/sce_sys/resources/kernel.skprx"
#define PLUGIN_USER_PATH "ux0:app/SAVSYNC01/sce_sys/resources/user.suprx"

__attribute__((weak)) void sqlite3_rw_init(void) {
    sqlite3_initialize();
}
__attribute__((weak)) void sqlite3_rw_exit(void) {
    sqlite3_shutdown();
}
__attribute__((weak)) void prevent_to_sleep(void) {}
__attribute__((weak)) void launch_app_by_title_id(const char *title_id) {}
#define SFO_MAGIC 0x46535000 // \x00PSF

struct sfo_header {
  uint32_t magic;
  uint32_t version;
  uint32_t key_table_offset;
  uint32_t data_table_offset;
  uint32_t entries;
};

struct sfo_index {
  uint16_t key_offset;
  uint16_t param_format;
  uint32_t param_length;
  uint32_t param_max_length;
  uint32_t data_offset;
};

typedef struct AppInfo {
  char *title_id; //[16];
  char *real_id;  //[16];
  char *name;     //[256];
  char *iconpath; //[256];
} AppInfo;

typedef struct AppList {
  int size;
  AppInfo *list;
} AppList;

static char pfs_mount_point[MAX_MOUNT_POINT_LENGTH];

static int known_pfs_ids[] = {
    0x6E,
    0x12E,
    0x12F,
    0x3ED,
};

static pthread_t *applist_t = NULL;
static AppList *applist = NULL;
static char *game_save_dirs[2] = {"ux0:user/00/savedata", "grw0:savedata"};

int taiLoad() {
  // https://github.com/TheOfficialFloW/VitaShell/blob/81af70971ba18b8ce86215b04180f1e3d21cdfc9/init.c#L401C1-L402C1
  int search_unk[2];
  SceUID search_modid =
      _vshKernelSearchModuleByName("VitaShellKernel2", search_unk);

  if (search_modid >= 0) {
    return search_modid;
  }

  return taiLoadStartKernelModule(PLUGIN_KERNEL_PATH, 0, NULL, 0);
}

int sceLoad() {
  int rc = sceKernelLoadStartModule(PLUGIN_USER_PATH, 0, NULL, 0, NULL, NULL);
  // Allow writing to ux0:app/SAVSYNC01
  sceAppMgrUmount("app0:");
  sceAppMgrUmount("savedata0:");
  return rc;
}

void sceAppUtilLoad() {
  // Init SceAppUtil
  SceAppUtilInitParam init_param;
  SceAppUtilBootParam boot_param;
  memset(&init_param, 0, sizeof(SceAppUtilInitParam));
  memset(&boot_param, 0, sizeof(SceAppUtilBootParam));
  sceAppUtilInit(&init_param, &boot_param);

  // Mount
  sceAppUtilMusicMount();
  sceAppUtilPhotoMount();
}

void sceAppUtilExit() {
  // Unmount
  sceAppUtilPhotoUmount();
  sceAppUtilMusicUmount();

  // Shutdown AppUtil
  sceAppUtilShutdown();
}

int pfs_mount(const char *path) {
  char klicensee[0x10];
  // char license_buf[0x200];
  ShellMountIdArgs args;

  memset(klicensee, 0, sizeof(klicensee));

  args.process_titleid = "SAVSYNC01";
  args.path = path;
  args.desired_mount_point = NULL;
  args.klicensee = klicensee;
  args.mount_point = pfs_mount_point;

  unsigned int i;
  for (i = 0; i < sizeof(known_pfs_ids) / sizeof(int); i++) {
    args.id = known_pfs_ids[i];

    int res = shellUserMountById(&args);
    if (res >= 0) {
      return res;
    }
  }

  return sceAppMgrGameDataMount(path, 0, 0, pfs_mount_point);
}

int pfs_unmount() {
  if (pfs_mount_point[0] == 0) {
    return -1;
  }

  int res = sceAppMgrUmount(pfs_mount_point);
  if (res >= 0) {
    memset(pfs_mount_point, 0, sizeof(pfs_mount_point));
  }

  return res;
}

static int dir_exists(char *path) {
  int rc = 0;
  struct stat st = {0};
  rc = stat(path, &st);

  if (rc != 0) {
    return rc;
  }

  if (S_ISDIR(st.st_mode)) {
    return 0;
  }

  return -1;
}

static int get_applist_callback(void *data, UNUSED(int argc), char **argv,
                                UNUSED(char **cols)) {
  // check if save exist
  char save_path_dir[40] = {0};
  int exists = -1;
  for (int idx = 0; idx < 2; idx++) {
    memset(save_path_dir, 0, 40);
    snprintf(save_path_dir, 40, "%s/%s", game_save_dirs[idx], argv[1]);
    exists = dir_exists(save_path_dir);
    if (exists == 0) {
      break;
    }
  }

  if (exists != 0) {
    return 0;
  }

  AppList *appList = (AppList *)data;
  if (appList->size == 0) {
    appList->list = calloc(1, sizeof(AppInfo));
  }
  AppInfo *appInfo = &appList->list[appList->size];

  appInfo->title_id = strdup(argv[0]);
  appInfo->real_id = strdup(argv[1]);
  appInfo->name = strdup(argv[2]);
  appInfo->iconpath = strdup(argv[3]);
  for (unsigned int i = 0; i < strlen(appInfo->name); i++) {
    if (appInfo->name[i] == '\n') {
      appInfo->name[i] = ' ';
    }
  }

  AppInfo *newP = realloc(appList->list, sizeof(AppInfo) * (appList->size + 2));
  appList->list = newP;
  appList->size += 1;

  return 0;
}

void applist_free(AppList *applist_ptr) {
  AppList *ptr = applist_ptr;
  if (applist_t != NULL) {
    pthread_join(*applist_t, NULL);
    free(applist_t);
    applist_t = NULL;
  }
  if (ptr == NULL) {
    if (applist != NULL) {
      ptr = applist;
      applist = NULL;
    } else {
      return;
    }
  }
  if (ptr->list != NULL) {
    for (int i = 0; i < ptr->size; i++) {
      free(ptr->list[i].title_id);
      free(ptr->list[i].real_id);
      free(ptr->list[i].iconpath);
      free(ptr->list[i].name);
    }
    free(ptr->list);
  }
  free(ptr);
}

static void *_applist_init(UNUSED(void *data)) {
  AppList *list = malloc(sizeof(AppInfo));
  list->size = 0;
  list->list = NULL;
  sceSysmoduleLoadModule(SCE_SYSMODULE_SQLITE);
  sqlite3_rw_init();
  char *query = "select a.titleid, b.realid, c.title,"
                "       e.iconpath"
                "  from (select titleid"
                "          from tbl_appinfo"
                "         where key = 566916785"
                "           and titleid like 'PCS%'"
                "         order by titleid) a,"
                "       (select titleid, val as realid"
                "          from tbl_appinfo"
                "         where key = 278217076) b,"
                "       tbl_appinfo_icon c,"
                "       (select titleid, iconpath"
                "          from tbl_appinfo_icon"
                "         where type = 0) e"
                " where a.titleid = b.titleid"
                "   and a.titleid = c.titleid"
                "   and a.titleid = e.titleid"
                " order by a.titleid";

  sqlite3 *db;
  int rc = sqlite3_open_v2(PSV_APP_DB, &db, SQLITE_OPEN_READONLY, NULL);
  if (rc == 0) {
    char *errMsg = NULL;
    rc = sqlite3_exec(db, query, get_applist_callback, (void *)list, &errMsg);
    if (rc != SQLITE_OK) {
      rc = -2;
    }
    // free errMsg
    if (errMsg != NULL) {
      sqlite3_free(errMsg);
    }
    sqlite3_close(db);
  }
  sqlite3_rw_exit();

  // ponytail: fallback — scan save dirs, only include real PS Vita game saves (PCS prefix)
  if (list->size == 0) {
    char *save_dirs[2] = {"ux0:user/00/savedata", "grw0:savedata"};
    for (int d = 0; d < 2; d++) {
      int dfd = sceIoDopen(save_dirs[d]);
      if (dfd < 0) continue;
      SceIoDirent entry;
      while (sceIoDread(dfd, &entry) > 0) {
        if (!SCE_S_ISDIR(entry.d_stat.st_mode)) continue;
        // skip homebrew and system dirs (only real game saves start with PCS)
        if (strncmp(entry.d_name, "PCS", 3) != 0) continue;
        AppInfo *info = realloc(list->list, (list->size + 1) * sizeof(AppInfo));
        if (!info) break;
        list->list = info;
        AppInfo *app = &list->list[list->size];
        app->title_id = strdup(entry.d_name);
        app->real_id = strdup(entry.d_name);
        app->name = strdup(entry.d_name);
        char iconpath_buf[64];
        snprintf(iconpath_buf, sizeof(iconpath_buf), "ux0:app/%s/sce_sys/icon0.png", entry.d_name);
        app->iconpath = strdup(iconpath_buf);
        list->size++;
      }
      sceIoDclose(dfd);
    }
  }

  if (rc != 0 && list->size == 0) {
    applist_free(list);
    list = NULL;
  }

  applist = list;
  return NULL;
}

void applist_init() {
  applist_t = malloc(sizeof(pthread_t));
  pthread_create(applist_t, NULL, _applist_init, NULL);
}

AppList *applist_get() {
  if (applist != NULL) {
    // free pthread
    if (applist_t != NULL) {
      pthread_join(*applist_t, NULL);
      free(applist_t);
      applist_t = NULL;
    }
    // return applist
    AppList *ptr = applist;
    applist = NULL;
    return ptr;
  }
  return NULL;
}

// below codes use part of vitashell codeset
uint64_t get_account_id() {
  uint64_t g_aid;
  if (sceRegMgrGetKeyBin("/CONFIG/NP", "account_id", &g_aid, sizeof(uint64_t)) <
      0) {
    return 0;
  }
  return g_aid;
}

int8_t change_account_id(const char *sfo_path, const uint64_t aid) {
  FILE *f = fopen(sfo_path, "r+b");
  // sfo_path not exist
  if (f == NULL) {
    return -1;
  }
  // read sfo
  struct sfo_header hdr = {0};
  fread(&hdr, sizeof(struct sfo_header), 1, f);
  if (hdr.magic != SFO_MAGIC) {
    // magic mismatch
    fclose(f);
    return -2;
  }
  int64_t data_offset = -1;
  for (uint32_t i = 0; i < hdr.entries; i++) {
    fseek(f, sizeof(struct sfo_header) + sizeof(struct sfo_index) * i,
          SEEK_SET);
    struct sfo_index idx = {0};
    fread(&idx, sizeof(struct sfo_index), 1, f);
    char key[64];
    fseek(f, hdr.key_table_offset + idx.key_offset, SEEK_SET);
    fread(&key, sizeof(char), 64, f);
    if (strncmp(key, "ACCOUNT_ID", 10) != 0) {
      continue;
    }
    data_offset = hdr.data_table_offset + idx.data_offset;
    break;
  }
  if (data_offset == -1) {
    // not exist ACCOUNT_ID
    fclose(f);
    return -3;
  }

  uint64_t old_aid;
  fseek(f, data_offset, SEEK_SET);
  fread(&old_aid, sizeof(uint64_t), 1, f);
  if (old_aid == aid) {
    fclose(f);
    // aid is already same
    return 1;
  }

  fseek(f, data_offset, SEEK_SET);
  fwrite(&aid, sizeof(uint64_t), 1, f);
  fclose(f);
  return 0;
}
