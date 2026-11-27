/* Replays a PrusaSlicer-family slicer's OctoPrint requests with libcurl,
 * through the same calls its src/slic3r/Utils/Http.cpp makes (curl_formadd,
 * CURLFORM_STREAM with CURLFORM_CONTENTSLENGTH, its user agent), so what
 * reaches the server is what the slicer sends. See README.md.
 *
 *   replay URL USER_AGENT API_KEY get
 *   replay URL USER_AGENT API_KEY post name=value... file=PATH:FILENAME
 *
 * Fields go in the order given, as the slicer adds them.
 */
#include <curl/curl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static size_t from_file(char *buf, size_t size, size_t n, void *f) {
	return fread(buf, size, n, (FILE *)f);
}

int main(int argc, char **argv) {
	if (argc < 5) {
		fprintf(stderr, "usage: replay URL USER_AGENT API_KEY get|post [fields]\n");
		return 2;
	}
	curl_global_init(CURL_GLOBAL_DEFAULT);
	CURL *c = curl_easy_init();
	struct curl_slist *headers = NULL;
	char key[512];
	snprintf(key, sizeof key, "X-Api-Key: %s", argv[3]);
	headers = curl_slist_append(headers, key);
	curl_easy_setopt(c, CURLOPT_URL, argv[1]);
	curl_easy_setopt(c, CURLOPT_USERAGENT, argv[2]);
	curl_easy_setopt(c, CURLOPT_HTTPHEADER, headers);
	curl_easy_setopt(c, CURLOPT_READFUNCTION, from_file);
	struct curl_httppost *form = NULL, *end = NULL;
	FILE *f = NULL;
	if (strcmp(argv[4], "post") == 0) {
		for (int i = 5; i < argc; i++) {
			char *eq = strchr(argv[i], '=');
			if (!eq) return 2;
			*eq = 0;
			if (strcmp(argv[i], "file") == 0) {
				char *colon = strrchr(eq + 1, ':');
				if (!colon) return 2;
				*colon = 0;
				f = fopen(eq + 1, "rb");
				if (!f) { perror(eq + 1); return 1; }
				fseek(f, 0, SEEK_END);
				long size = ftell(f);
				fseek(f, 0, SEEK_SET);
				curl_formadd(&form, &end,
					CURLFORM_COPYNAME, "file",
					CURLFORM_FILENAME, colon + 1,
					CURLFORM_CONTENTTYPE, "application/octet-stream",
					CURLFORM_STREAM, (void *)f,
					CURLFORM_CONTENTSLENGTH, size,
					CURLFORM_END);
			} else {
				curl_formadd(&form, &end,
					CURLFORM_COPYNAME, argv[i],
					CURLFORM_COPYCONTENTS, eq + 1,
					CURLFORM_END);
			}
		}
		curl_easy_setopt(c, CURLOPT_HTTPPOST, form);
	}
	CURLcode r = curl_easy_perform(c);
	long status = 0;
	curl_easy_getinfo(c, CURLINFO_RESPONSE_CODE, &status);
	fprintf(stderr, "%s: HTTP %ld\n", curl_easy_strerror(r), status);
	return r == CURLE_OK && status / 100 == 2 ? 0 : 1;
}
