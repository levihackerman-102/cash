#include <stdlib.h>
#include <stdio.h>

#define CASH_RL_BUFSIZE 1024

char *cash_read_line(void) {
  int bufsize = CASH_RL_BUFSIZE;
  int position = 0;
  char *buffer = malloc(sizeof(char) * bufsize);
  int c;

  if (!buffer) {
    fprintf(stderr, "cash: allocation error\n");
    exit(EXIT_FAILURE);
  }

  while (1) {
    // Read a character
    c = getchar();

    // If we hit EOF, replace it with a null character and return.
    if (c == EOF || c == '\n') {
      buffer[position] = '\0';
      return buffer;
    } else {
      buffer[position] = c;
    }
    position++;

    // If we have exceeded the buffer, reallocate.
    if (position >= bufsize) {
      bufsize += CASH_RL_BUFSIZE;
      buffer = realloc(buffer, bufsize);
      if (!buffer) {
        fprintf(stderr, "cash: allocation error\n");
        exit(EXIT_FAILURE);
      }
    }
  }
}


#define CASH_TOK_BUFSIZE 64
#define CASH_TOK_DELIM " \t\r\n\a"

char **cash_split_line(char *line)
{
  int bufsize = CASH_TOK_BUFSIZE, position = 0;
  char **tokens = malloc(bufsize * sizeof(char*));
  char *token;

  if (!tokens) {
    fprintf(stderr, "cash: allocation error\n");
    exit(EXIT_FAILURE);
  }

  token = strtok(line, CASH_TOK_DELIM);
  while (token != NULL) {
    tokens[position] = token;
    position++;

    if (position >= bufsize) {
      bufsize += CASH_TOK_BUFSIZE;
      tokens = realloc(tokens, bufsize * sizeof(char*));
      if (!tokens) {
        fprintf(stderr, "cash: allocation error\n");
        exit(EXIT_FAILURE);
      }
    }

    token = strtok(NULL, CASH_TOK_DELIM);
  }
  tokens[position] = NULL;
  return tokens;
}


void cash_loop() {
    char *line;
    char **args;
    int status;

    do {
        printf("> ");
        line = cash_read_line();
        args = cash_split_line(line);
        status = cash_execute(args);

        free(line);
        free(args);
    } while (status);
}


int cash_launch(char **args)
{
  pid_t pid, wpid;
  int status;

  pid = fork();
  if (pid == 0) {
    // Child process
    if (execvp(args[0], args) == -1) {
      perror("cash");
    }
    exit(EXIT_FAILURE);
  } else if (pid < 0) {
    // Error forking
    perror("cash");
  } else {
    // Parent process
    do {
      wpid = waitpid(pid, &status, WUNTRACED);
    } while (!WIFEXITED(status) && !WIFSIGNALED(status));
  }

  return 1;
}

int main(int argc, char **argv)
{
  // Load config files

  // Run command loop.
  cash_loop();

  // Perform shutdown/cleanup.

  return EXIT_SUCCESS;
}
